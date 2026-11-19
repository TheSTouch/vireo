use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow};
use futures::channel::mpsc;
use futures::{AsyncBufReadExt as _, AsyncWriteExt as _, StreamExt as _, io::BufReader};
use gpui::{BackgroundExecutor, Task};
use util::command::{Child, Stdio, new_command};

use crate::protocol::{CliMessage, PermissionMode, StdinMessage};

/// The value Vireo reports in `CLAUDE_CODE_ENTRYPOINT`, so that the CLI's
/// telemetry and behaviour treat it as an IDE panel rather than a terminal.
pub const ENTRYPOINT: &str = "claude-vireo";
pub const DEFAULT_MAX_THINKING_TOKENS: u32 = 31999;

/// Everything needed to start one `claude` process for one conversation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LaunchOptions {
    pub cwd: PathBuf,
    pub resume: Option<String>,
    pub fork_session: bool,
    pub permission_mode: Option<PermissionMode>,
    pub model: Option<String>,
    pub max_thinking_tokens: Option<u32>,
    pub allow_dangerously_skip_permissions: bool,
    pub environment: Vec<(String, String)>,
    pub process_wrapper: Option<PathBuf>,
    pub python_environment: Option<PythonEnvironment>,
    pub ide_port: Option<u16>,
    /// The name of an in-process MCP server that Vireo answers through
    /// `mcp_message` control requests.
    pub sdk_mcp_server: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PythonEnvironment {
    pub root: PathBuf,
    pub bin_dir: PathBuf,
}

/// The program, arguments and extra environment of a `claude` launch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandLine {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: PathBuf,
}

/// Finds the user's installed `claude`. Vireo never bundles it.
///
/// The order matches the CLI's own install layouts: the local install that
/// `claude migrate-installer` creates, then `PATH`, then the native
/// installer's location.
pub fn find_claude(home: Option<&Path>, path_env: Option<OsString>) -> Option<PathBuf> {
    let executable = if cfg!(windows) {
        "claude.exe"
    } else {
        "claude"
    };
    if let Some(home) = home {
        let local = home.join(".claude").join("local").join(executable);
        if local.is_file() {
            return Some(local);
        }
    }
    if let Some(path_env) = path_env {
        for dir in std::env::split_paths(&path_env) {
            let candidate = dir.join(executable);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    if let Some(home) = home {
        let native = home.join(".local").join("bin").join(executable);
        if native.is_file() {
            return Some(native);
        }
    }
    None
}

/// Finds a virtual environment at the root of the workspace, the layouts
/// `python -m venv`, uv and poetry (in-project) create.
pub fn find_python_environment(worktree_root: &Path) -> Option<PythonEnvironment> {
    let bin = if cfg!(windows) { "Scripts" } else { "bin" };
    [".venv", "venv", "env", ".env"]
        .iter()
        .map(|name| worktree_root.join(name))
        .find(|root| root.join("pyvenv.cfg").is_file() && root.join(bin).is_dir())
        .map(|root| PythonEnvironment {
            bin_dir: root.join(bin),
            root,
        })
}

pub fn build_command_line(
    claude: &Path,
    options: &LaunchOptions,
    inherited_path: Option<OsString>,
) -> CommandLine {
    let mut args = vec![
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        "--input-format".to_string(),
        "stream-json".to_string(),
        format!(
            "--max-thinking-tokens={}",
            options
                .max_thinking_tokens
                .unwrap_or(DEFAULT_MAX_THINKING_TOKENS)
        ),
        "--permission-prompt-tool=stdio".to_string(),
    ];
    if let Some(resume) = &options.resume {
        args.push(format!("--resume={resume}"));
        if options.fork_session {
            args.push("--fork-session".to_string());
        }
    }
    args.push("--setting-sources=user,project,local".to_string());
    if let Some(mode) = options.permission_mode {
        args.push(format!("--permission-mode={}", mode.as_cli_str()));
    }
    if options.allow_dangerously_skip_permissions {
        args.push("--allow-dangerously-skip-permissions".to_string());
    }
    if let Some(model) = &options.model {
        args.push(format!("--model={model}"));
    }
    if let Some(server) = &options.sdk_mcp_server {
        let config = serde_json::json!({
            "mcpServers": {server.as_str(): {"type": "sdk", "name": server}}
        });
        args.push(format!("--mcp-config={config}"));
    }
    args.extend(
        [
            "--include-partial-messages",
            "--debug-to-stderr",
            "--replay-user-messages",
        ]
        .map(String::from),
    );

    let mut env = BTreeMap::new();
    env.insert("CLAUDE_CODE_ENTRYPOINT".to_string(), ENTRYPOINT.to_string());
    env.insert(
        "CLAUDE_CODE_SDK_READS_SESSION_STATE".to_string(),
        "1".to_string(),
    );
    env.insert(
        "CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING".to_string(),
        "true".to_string(),
    );
    env.insert("MCP_CONNECTION_NONBLOCKING".to_string(), "true".to_string());
    if let Some(port) = options.ide_port {
        env.insert("CLAUDE_CODE_SSE_PORT".to_string(), port.to_string());
        env.insert("ENABLE_IDE_INTEGRATION".to_string(), "true".to_string());
    }
    if let Some(python) = &options.python_environment {
        let mut paths = vec![python.bin_dir.clone()];
        if let Some(inherited) = &inherited_path {
            paths.extend(std::env::split_paths(inherited));
        }
        if let Ok(joined) = std::env::join_paths(paths) {
            env.insert("PATH".to_string(), joined.to_string_lossy().into_owned());
        }
        env.insert(
            "VIRTUAL_ENV".to_string(),
            python.root.to_string_lossy().into_owned(),
        );
    }
    // The user's variables come last so they can override anything above.
    for (name, value) in &options.environment {
        env.insert(name.clone(), value.clone());
    }

    let (program, args) = match &options.process_wrapper {
        Some(wrapper) => {
            let mut wrapped = vec![claude.to_string_lossy().into_owned()];
            wrapped.extend(args);
            (wrapper.clone(), wrapped)
        }
        None => (claude.to_path_buf(), args),
    };

    CommandLine {
        program,
        args,
        env,
        cwd: options.cwd.clone(),
    }
}

#[derive(Debug)]
pub enum ProcessEvent {
    Message(CliMessage),
    /// A stdout line that is not valid protocol JSON.
    Unparsed(String),
    Stderr(String),
    Exited(Option<i32>),
}

/// A running `claude` process. Dropping it kills the process.
pub struct ClaudeProcess {
    stdin: mpsc::UnboundedSender<String>,
    child: Child,
    _io: Task<()>,
}

impl ClaudeProcess {
    pub fn spawn(
        command_line: &CommandLine,
        executor: &BackgroundExecutor,
    ) -> Result<(Self, mpsc::UnboundedReceiver<ProcessEvent>)> {
        let mut command = new_command(&command_line.program);
        command
            .args(&command_line.args)
            .envs(&command_line.env)
            // Vireo's own terminal sets this, and a nested `claude` would refuse to start.
            .env_remove("CLAUDECODE")
            .current_dir(&command_line.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().with_context(|| {
            format!(
                "failed to start Claude Code at {}",
                command_line.program.display()
            )
        })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("no stdout from claude"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("no stderr from claude"))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("no stdin to claude"))?;

        let (events_tx, events_rx) = mpsc::unbounded();
        let (stdin_tx, mut stdin_rx) = mpsc::unbounded::<String>();

        let io = executor.spawn({
            async move {
                let read_stdout = {
                    let events_tx = events_tx.clone();
                    async move {
                        let mut lines = BufReader::new(stdout).lines();
                        while let Some(line) = lines.next().await {
                            let Ok(line) = line else { break };
                            if line.trim().is_empty() {
                                continue;
                            }
                            let event = match CliMessage::parse(&line) {
                                Ok(message) => ProcessEvent::Message(message),
                                Err(error) => {
                                    log::debug!("unparsed claude output ({error}): {line}");
                                    ProcessEvent::Unparsed(line)
                                }
                            };
                            if events_tx.unbounded_send(event).is_err() {
                                break;
                            }
                        }
                    }
                };
                let read_stderr = {
                    let events_tx = events_tx.clone();
                    async move {
                        let mut lines = BufReader::new(stderr).lines();
                        while let Some(Ok(line)) = lines.next().await {
                            if events_tx
                                .unbounded_send(ProcessEvent::Stderr(line))
                                .is_err()
                            {
                                break;
                            }
                        }
                    }
                };
                let write_stdin = async move {
                    while let Some(line) = stdin_rx.next().await {
                        if let Err(error) = stdin.write_all(line.as_bytes()).await {
                            log::warn!("failed to write to claude: {error}");
                            break;
                        }
                        if let Err(error) = stdin.flush().await {
                            log::warn!("failed to flush claude stdin: {error}");
                            break;
                        }
                    }
                };
                futures::join!(read_stdout, read_stderr, write_stdin);
            }
        });

        Ok((
            Self {
                stdin: stdin_tx,
                child,
                _io: io,
            },
            events_rx,
        ))
    }

    pub fn send(&self, message: &StdinMessage) -> Result<()> {
        let line = message.to_line()?;
        self.stdin
            .unbounded_send(line)
            .map_err(|_| anyhow!("Claude Code is no longer running"))
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Returns the exit code once the process has ended.
    pub fn try_exit_status(&mut self) -> Option<Option<i32>> {
        match self.child.try_status() {
            Ok(Some(status)) => Some(status.code()),
            Ok(None) => None,
            Err(error) => {
                log::warn!("failed to poll claude: {error}");
                Some(None)
            }
        }
    }

    pub fn kill(&mut self) {
        if let Err(error) = self.child.kill() {
            log::debug!("failed to kill claude: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> LaunchOptions {
        LaunchOptions {
            cwd: PathBuf::from("/project"),
            ..Default::default()
        }
    }

    #[test]
    fn command_line_matches_the_extension_launch() {
        let line = build_command_line(Path::new("/bin/claude"), &options(), None);
        assert_eq!(line.program, PathBuf::from("/bin/claude"));
        assert_eq!(
            line.args,
            vec![
                "--output-format",
                "stream-json",
                "--verbose",
                "--input-format",
                "stream-json",
                "--max-thinking-tokens=31999",
                "--permission-prompt-tool=stdio",
                "--setting-sources=user,project,local",
                "--include-partial-messages",
                "--debug-to-stderr",
                "--replay-user-messages",
            ]
        );
        assert_eq!(line.env["CLAUDE_CODE_ENTRYPOINT"], ENTRYPOINT);
        assert!(!line.env.contains_key("CLAUDE_CODE_SSE_PORT"));
        assert_eq!(line.cwd, PathBuf::from("/project"));
    }

    #[test]
    fn command_line_resume_mode_model_and_ide() {
        let options = LaunchOptions {
            resume: Some("abc".into()),
            permission_mode: Some(PermissionMode::Plan),
            model: Some("opus".into()),
            allow_dangerously_skip_permissions: true,
            ide_port: Some(4242),
            ..options()
        };
        let line = build_command_line(Path::new("/bin/claude"), &options, None);
        assert!(line.args.contains(&"--resume=abc".to_string()));
        assert!(line.args.contains(&"--permission-mode=plan".to_string()));
        assert!(line.args.contains(&"--model=opus".to_string()));
        assert!(
            line.args
                .contains(&"--allow-dangerously-skip-permissions".to_string())
        );
        assert_eq!(line.env["CLAUDE_CODE_SSE_PORT"], "4242");
        assert_eq!(line.env["ENABLE_IDE_INTEGRATION"], "true");

        let options = LaunchOptions {
            sdk_mcp_server: Some("vireo".into()),
            ..LaunchOptions::default()
        };
        let line = build_command_line(Path::new("/bin/claude"), &options, None);
        assert!(line.args.contains(
            &r#"--mcp-config={"mcpServers":{"vireo":{"type":"sdk","name":"vireo"}}}"#.to_string()
        ));
    }

    #[test]
    fn command_line_wrapper_and_user_environment() {
        let options = LaunchOptions {
            process_wrapper: Some(PathBuf::from("/usr/bin/wrap")),
            environment: vec![
                ("FOO".into(), "bar".into()),
                ("CLAUDE_CODE_ENTRYPOINT".into(), "mine".into()),
            ],
            ..options()
        };
        let line = build_command_line(Path::new("/bin/claude"), &options, None);
        assert_eq!(line.program, PathBuf::from("/usr/bin/wrap"));
        assert_eq!(line.args[0], "/bin/claude");
        assert_eq!(line.args[1], "--output-format");
        assert_eq!(line.env["FOO"], "bar");
        assert_eq!(line.env["CLAUDE_CODE_ENTRYPOINT"], "mine");
    }

    #[test]
    fn command_line_python_environment() {
        let options = LaunchOptions {
            python_environment: Some(PythonEnvironment {
                root: PathBuf::from("/project/.venv"),
                bin_dir: PathBuf::from("/project/.venv/bin"),
            }),
            ..options()
        };
        let line = build_command_line(
            Path::new("/bin/claude"),
            &options,
            Some(OsString::from("/usr/bin")),
        );
        assert_eq!(line.env["VIRTUAL_ENV"], "/project/.venv");
        let paths: Vec<PathBuf> = std::env::split_paths(&line.env["PATH"]).collect();
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/project/.venv/bin"),
                PathBuf::from("/usr/bin")
            ]
        );
    }

    #[test]
    fn finds_claude_and_python_environment() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(home.join(".claude/local")).unwrap();
        let executable = if cfg!(windows) {
            "claude.exe"
        } else {
            "claude"
        };
        assert_eq!(find_claude(Some(&home), Some(bin.clone().into())), None);

        std::fs::write(bin.join(executable), "").unwrap();
        assert_eq!(
            find_claude(Some(&home), Some(bin.clone().into())),
            Some(bin.join(executable))
        );
        let local = home.join(".claude/local").join(executable);
        std::fs::write(&local, "").unwrap();
        assert_eq!(find_claude(Some(&home), Some(bin.into())), Some(local));

        let project = dir.path().join("project");
        assert_eq!(find_python_environment(&project), None);
        let bin_name = if cfg!(windows) { "Scripts" } else { "bin" };
        std::fs::create_dir_all(project.join(".venv").join(bin_name)).unwrap();
        std::fs::write(project.join(".venv/pyvenv.cfg"), "").unwrap();
        assert_eq!(
            find_python_environment(&project),
            Some(PythonEnvironment {
                root: project.join(".venv"),
                bin_dir: project.join(".venv").join(bin_name),
            })
        );
    }
}
