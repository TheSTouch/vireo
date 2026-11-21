use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow};
use chrono::{DateTime, Utc};
use futures::StreamExt as _;
use futures::channel::{mpsc, oneshot};
use gpui::{App, AppContext as _, Context, Entity, EventEmitter, Global, Task, WeakEntity};
use project::Project;
use serde_json::{Value, json};
use settings::Settings as _;

use crate::conversation::{Conversation, HostRequest, NoticeKind, PermissionStatus};
use crate::process::{
    ClaudeProcess, LaunchOptions, ProcessEvent, build_command_line, find_claude,
    find_python_environment,
};
use crate::protocol::{
    CliMessage, ContentBlock, ImageSource, InitializeResponse, OutgoingControlRequest,
    OutgoingControlResponse, PermissionDecision, PermissionMode, StdinMessage,
};
use crate::settings::ClaudeCodeSettings;

/// The in-process MCP server through which a panel session gets Vireo's IDE
/// tools, as `mcp__vireo__getDiagnostics` and `mcp__vireo__executeCode`.
pub const SDK_MCP_SERVER: &str = "vireo";
const AUTOSAVE_HOOK_ID: &str = "vireo_autosave";
const AUTOSAVE_TOOLS: &str = "Read|Edit|MultiEdit|Write|NotebookEdit";
const MAX_LOG_LINES: usize = 5000;
/// Sent to resume a turn that a restart of the process interrupted.
pub const CONTINUE_PROMPT: &str = "Continue from where you left off.";
pub const CONTINUED_NOTICE: &str = "Claude Code restarted; continuing the interrupted step.";

#[cfg(test)]
type FakeSpawner = std::rc::Rc<
    dyn Fn(&LaunchOptions) -> (ClaudeProcess, mpsc::UnboundedReceiver<ProcessEvent>),
>;

/// The CLI's debug output and Vireo's own notes about the process, shown by
/// "Show Logs".
#[derive(Default)]
pub struct ClaudeLogs {
    pub lines: VecDeque<String>,
}

impl Global for ClaudeLogs {}

impl ClaudeLogs {
    pub fn push(line: impl Into<String>, cx: &mut App) {
        let logs = cx.default_global::<ClaudeLogs>();
        if logs.lines.len() >= MAX_LOG_LINES {
            logs.lines.pop_front();
        }
        logs.lines.push_back(line.into());
    }

    pub fn text(cx: &App) -> String {
        cx.try_global::<ClaudeLogs>()
            .map(|logs| logs.lines.iter().cloned().collect::<Vec<_>>().join("\n"))
            .unwrap_or_default()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ProcessStatus {
    NotStarted,
    Running,
    Exited(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum SessionEvent {
    Updated,
    PermissionRequested { request_id: String },
    TurnFinished,
    Exited,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PromptImage {
    pub media_type: String,
    pub base64: String,
}

pub struct ClaudeSession {
    pub conversation: Conversation,
    pub status: ProcessStatus,
    pub initialize: Option<InitializeResponse>,
    pub last_activity: DateTime<Utc>,
    pub cwd: PathBuf,
    project: WeakEntity<Project>,
    process: Option<ClaudeProcess>,
    pending_requests: HashMap<String, oneshot::Sender<Result<Value>>>,
    request_counter: u64,
    resume: Option<String>,
    fork_session: bool,
    resume_session_at: Option<String>,
    model: Option<String>,
    ide: Option<WeakEntity<claude_ide::ClaudeIde>>,
    /// Counts the processes started, so that the end of one that was
    /// replaced is not taken for the end of the current one.
    process_generation: u64,
    /// Whether Claude has answered at all in the current turn: a turn that
    /// was cut short after that is continued when the process restarts, one
    /// that never got going is not, so a process that keeps failing is not
    /// restarted over and over.
    turn_progressed: bool,
    #[cfg(test)]
    fake_spawner: Option<FakeSpawner>,
    _events: Option<Task<()>>,
}

impl EventEmitter<SessionEvent> for ClaudeSession {}

impl ClaudeSession {
    pub fn new(
        project: WeakEntity<Project>,
        cwd: PathBuf,
        resume: Option<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut conversation = Conversation::default();
        conversation.session_id = resume.clone();
        conversation.permission_mode = ClaudeCodeSettings::get_global(cx).initial_permission_mode;
        Self {
            conversation,
            status: ProcessStatus::NotStarted,
            initialize: None,
            last_activity: Utc::now(),
            cwd,
            project,
            process: None,
            pending_requests: HashMap::new(),
            request_counter: 0,
            resume,
            fork_session: false,
            resume_session_at: None,
            model: None,
            ide: None,
            process_generation: 0,
            turn_progressed: false,
            #[cfg(test)]
            fake_spawner: None,
            _events: None,
        }
    }

    #[cfg(test)]
    pub fn set_fake_spawner(&mut self, spawner: FakeSpawner) {
        self.fake_spawner = Some(spawner);
    }

    fn spawn_process(
        &self,
        options: &LaunchOptions,
        cx: &mut Context<Self>,
    ) -> Result<(ClaudeProcess, mpsc::UnboundedReceiver<ProcessEvent>)> {
        #[cfg(test)]
        {
            if let Some(spawner) = &self.fake_spawner {
                return Ok(spawner(options));
            }
        }
        if cx.is_test() {
            anyhow::bail!("tests run Claude Code with a fake process");
        }
        let claude = find_claude(paths::home_dir().as_path().into(), std::env::var_os("PATH"))
            .context(
                "Claude Code is not installed. Install it with `npm install -g @anthropic-ai/claude-code` or from https://claude.com/claude-code, then reopen this panel.",
            )?;
        let command_line = build_command_line(&claude, options, std::env::var_os("PATH"));
        ClaudeLogs::push(
            format!(
                "[vireo] starting {} {}",
                command_line.program.display(),
                command_line.args.join(" ")
            ),
            cx,
        );
        ClaudeProcess::spawn(&command_line, cx.background_executor())
    }

    pub fn session_id(&self) -> Option<&str> {
        self.conversation.session_id.as_deref()
    }

    pub fn project(&self) -> Option<Entity<Project>> {
        self.project.upgrade()
    }

    pub fn is_running(&self) -> bool {
        self.process.is_some()
    }

    /// Connects the session to the IDE server of its window, which answers the
    /// session's `mcp__vireo__*` tool calls.
    pub fn set_ide(&mut self, ide: Option<WeakEntity<claude_ide::ClaudeIde>>) {
        self.ide = ide;
    }

    pub fn set_fork_on_start(&mut self, fork: bool) {
        self.fork_session = fork;
    }

    /// Forks only the part of the session up to this message.
    pub fn set_resume_session_at(&mut self, message_id: Option<String>) {
        self.resume_session_at = message_id;
    }

    fn launch_options(&self, cx: &App) -> LaunchOptions {
        let settings = ClaudeCodeSettings::get_global(cx);
        let python_environment = if settings.use_python_environment {
            find_python_environment(&self.cwd)
        } else {
            None
        };
        LaunchOptions {
            cwd: self.cwd.clone(),
            resume: self.conversation.session_id.clone().or(self.resume.clone()),
            fork_session: self.fork_session,
            resume_session_at: self.resume_session_at.clone(),
            permission_mode: self.conversation.permission_mode,
            model: self.model.clone(),
            max_thinking_tokens: None,
            allow_dangerously_skip_permissions: settings.allow_dangerously_skip_permissions,
            environment: settings.environment_variables.clone(),
            process_wrapper: settings.claude_process_wrapper.as_ref().map(PathBuf::from),
            python_environment,
            ide_port: None,
            sdk_mcp_server: self.ide.is_some().then(|| SDK_MCP_SERVER.to_string()),
        }
    }

    /// Starts the `claude` process if it is not running.
    pub fn start(&mut self, cx: &mut Context<Self>) -> Result<()> {
        if self.process.is_some() {
            return Ok(());
        }
        let options = self.launch_options(cx);
        let (process, events) = self.spawn_process(&options, cx)?;
        self.process = Some(process);
        self.process_generation += 1;
        let generation = self.process_generation;
        self.status = ProcessStatus::Running;
        self.fork_session = false;
        self.resume_session_at = None;
        self._events = Some(cx.spawn(async move |this, cx| {
            Self::pump_events(this, generation, events, cx).await;
        }));

        let settings = ClaudeCodeSettings::get_global(cx);
        let hooks = settings.autosave.then(|| {
            json!({
                "PreToolUse": [{"matcher": AUTOSAVE_TOOLS, "hookCallbackIds": [AUTOSAVE_HOOK_ID]}]
            })
        });
        let initialize = self.request(
            OutgoingControlRequest::Initialize {
                hooks,
                prompt_suggestions: settings.prompt_suggestions,
            },
            cx,
        );
        cx.spawn(async move |this, cx| {
            let response = initialize.await;
            this.update(cx, |this, cx| {
                match response.and_then(|value| {
                    serde_json::from_value::<InitializeResponse>(value).map_err(Into::into)
                }) {
                    Ok(response) => {
                        if this.conversation.permission_mode.is_none() {
                            this.conversation.permission_mode = response
                                .current_permission_mode
                                .as_deref()
                                .and_then(PermissionMode::from_cli_str);
                        }
                        this.initialize = Some(response);
                    }
                    Err(error) => {
                        ClaudeLogs::push(format!("[vireo] initialize failed: {error:#}"), cx)
                    }
                }
                cx.emit(SessionEvent::Updated);
                cx.notify();
            })
        })
        .detach();
        Ok(())
    }

    async fn pump_events(
        this: WeakEntity<Self>,
        generation: u64,
        mut events: mpsc::UnboundedReceiver<ProcessEvent>,
        cx: &mut gpui::AsyncApp,
    ) {
        while let Some(event) = events.next().await {
            if matches!(event, ProcessEvent::Exited(_)) {
                break;
            }
            let result = this.update(cx, |this, cx| this.handle_process_event(event, cx));
            if result.is_err() {
                return;
            }
        }
        this.update(cx, |this, cx| this.handle_exit(generation, cx))
            .ok();
    }

    fn handle_exit(&mut self, generation: u64, cx: &mut Context<Self>) {
        if generation != self.process_generation || self.process.is_none() {
            return;
        }
        let code = self
            .process
            .as_mut()
            .and_then(|process| process.try_exit_status())
            .flatten();
        self.process = None;
        self._events = None;
        for (_, sender) in self.pending_requests.drain() {
            sender.send(Err(anyhow!("Claude Code exited"))).ok();
        }
        let message = match code {
            Some(0) | None => "Claude Code stopped".to_string(),
            Some(code) => {
                format!("Claude Code exited with code {code}. See Show Logs for details.")
            }
        };
        let was_busy = self.conversation.turn != crate::conversation::TurnState::Idle;
        let continue_turn = was_busy && self.turn_progressed && self.may_continue(cx);
        if !continue_turn && (was_busy || !matches!(code, Some(0) | None)) {
            self.conversation
                .push_notice(NoticeKind::Error, message.clone(), Utc::now());
        }
        self.conversation.turn = crate::conversation::TurnState::Idle;
        ClaudeLogs::push(format!("[vireo] {message}"), cx);
        self.status = ProcessStatus::Exited(message);
        cx.emit(SessionEvent::Exited);
        if continue_turn {
            self.continue_interrupted_turn(cx);
        }
        cx.notify();
    }

    /// Whether a turn the process left unfinished may be continued on its
    /// own: plan mode waits for the person, and without a session id there is
    /// nothing to resume.
    pub fn may_continue(&self, cx: &App) -> bool {
        ClaudeCodeSettings::get_global(cx).continue_after_reload
            && self.conversation.permission_mode != Some(PermissionMode::Plan)
            && self.session_id().is_some()
    }

    /// Restarts the process on the session and asks Claude to go on with the
    /// step that was interrupted.
    pub fn continue_interrupted_turn(&mut self, cx: &mut Context<Self>) {
        self.conversation
            .push_notice(NoticeKind::Info, CONTINUED_NOTICE, Utc::now());
        ClaudeLogs::push("[vireo] continuing the interrupted step", cx);
        if let Err(error) = self.send_prompt(CONTINUE_PROMPT.to_string(), Vec::new(), cx) {
            log::warn!("failed to continue the Claude session: {error:#}");
        }
    }

    fn handle_process_event(&mut self, event: ProcessEvent, cx: &mut Context<Self>) {
        match event {
            ProcessEvent::Message(CliMessage::ControlResponse(response)) => {
                let body = response.response;
                if let Some(sender) = self.pending_requests.remove(&body.request_id) {
                    let result = if body.subtype == "error" {
                        Err(anyhow!(
                            body.error.unwrap_or_else(|| "request failed".into())
                        ))
                    } else {
                        Ok(body.response.unwrap_or(Value::Null))
                    };
                    sender.send(result).ok();
                }
            }
            ProcessEvent::Message(message) => {
                let was_busy = self.conversation.turn != crate::conversation::TurnState::Idle;
                if was_busy
                    && matches!(
                        message,
                        CliMessage::Assistant(_) | CliMessage::StreamEvent(_)
                    )
                {
                    self.turn_progressed = true;
                }
                let permission_request_id = match &message {
                    CliMessage::ControlRequest(request)
                        if matches!(
                            request.request,
                            crate::protocol::IncomingControlRequestBody::CanUseTool(_)
                        ) =>
                    {
                        Some(request.request_id.clone())
                    }
                    _ => None,
                };
                self.last_activity = Utc::now();
                if let Some(host_request) = self.conversation.apply(message, Utc::now()) {
                    self.handle_host_request(host_request, cx);
                }
                if let Some(request_id) = permission_request_id {
                    cx.emit(SessionEvent::PermissionRequested { request_id });
                }
                if was_busy && self.conversation.turn == crate::conversation::TurnState::Idle {
                    cx.emit(SessionEvent::TurnFinished);
                }
                cx.emit(SessionEvent::Updated);
                cx.notify();
            }
            ProcessEvent::Unparsed(line) => ClaudeLogs::push(format!("[stdout] {line}"), cx),
            ProcessEvent::Stderr(line) => ClaudeLogs::push(line, cx),
            // pump_events ends on it and calls handle_exit.
            ProcessEvent::Exited(_) => {}
        }
    }

    fn handle_host_request(&mut self, request: HostRequest, cx: &mut Context<Self>) {
        match request {
            HostRequest::HookCallback {
                request_id,
                callback_id,
                input,
            } => {
                if callback_id != AUTOSAVE_HOOK_ID {
                    self.respond(request_id, Ok(json!({})));
                    return;
                }
                let save = self.save_file_before_tool(&input, cx);
                cx.spawn(async move |this, cx| {
                    if let Err(error) = save.await {
                        log::warn!("autosave before Claude's tool failed: {error:#}");
                    }
                    this.update(cx, |this, _| this.respond(request_id, Ok(json!({}))))
                        .ok();
                })
                .detach();
            }
            HostRequest::McpMessage {
                request_id,
                server_name,
                message,
            } => self.handle_mcp_message(request_id, server_name, message, cx),
            HostRequest::Unsupported { request_id } => {
                self.respond(request_id, Err(anyhow!("Unsupported control request")));
            }
        }
    }

    fn handle_mcp_message(
        &mut self,
        request_id: String,
        server_name: String,
        message: Value,
        cx: &mut Context<Self>,
    ) {
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        let ide = self.ide.as_ref().and_then(WeakEntity::upgrade);
        let (true, Some(ide)) = (server_name == SDK_MCP_SERVER, ide) else {
            self.respond(
                request_id,
                Ok(json!({"mcp_response": claude_ide::error_response(
                    id,
                    -32601,
                    &format!("Server '{server_name}' not found"),
                )})),
            );
            return;
        };
        let version = release_channel::AppVersion::global(cx).to_string();
        match claude_ide::route_sdk_message(&message, &version) {
            claude_ide::Route::Respond(response) => {
                self.respond(request_id, Ok(json!({"mcp_response": response})));
            }
            claude_ide::Route::CallTool {
                id,
                name,
                arguments,
            } => {
                let task = ide.update(cx, |ide, cx| ide.call_tool(&name, arguments, cx));
                cx.spawn(async move |this, cx| {
                    let result = task.await;
                    this.update(cx, |this, _| {
                        this.respond(
                            request_id,
                            Ok(json!({"mcp_response": claude_ide::tool_response(id, &result)})),
                        )
                    })
                    .ok();
                })
                .detach();
            }
            claude_ide::Route::Connected { .. } | claude_ide::Route::Ignore => {
                self.respond(
                    request_id,
                    Ok(json!({"mcp_response": {"jsonrpc": "2.0", "result": {}}})),
                );
            }
        }
    }

    /// Saves the open buffer of the file a tool is about to read or write, so
    /// that Claude never works from a stale copy on disk.
    fn save_file_before_tool(
        &self,
        hook_input: &Value,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        let Some(path) = hook_input
            .get("tool_input")
            .and_then(|input| {
                input
                    .get("file_path")
                    .or_else(|| input.get("notebook_path"))
            })
            .and_then(Value::as_str)
            .map(PathBuf::from)
        else {
            return Task::ready(Ok(()));
        };
        let Some(project) = self.project.upgrade() else {
            return Task::ready(Ok(()));
        };
        save_dirty_buffer(&project, &path, cx)
    }

    fn respond(&self, request_id: String, result: Result<Value>) {
        let response = match result {
            Ok(response) => OutgoingControlResponse::Success {
                request_id,
                response,
            },
            Err(error) => OutgoingControlResponse::Error {
                request_id,
                error: format!("{error:#}"),
            },
        };
        if let Err(error) = self.send(&StdinMessage::ControlResponse { response }) {
            log::debug!("could not answer Claude Code: {error:#}");
        }
    }

    fn send(&self, message: &StdinMessage) -> Result<()> {
        self.process
            .as_ref()
            .ok_or_else(|| anyhow!("Claude Code is not running"))?
            .send(message)
    }

    fn next_request_id(&mut self) -> String {
        self.request_counter += 1;
        format!("req_{}_{:08x}", self.request_counter, rand::random::<u32>())
    }

    pub fn request(
        &mut self,
        request: OutgoingControlRequest,
        cx: &mut Context<Self>,
    ) -> Task<Result<Value>> {
        let request_id = self.next_request_id();
        let (sender, receiver) = oneshot::channel();
        if let Err(error) = self.send(&StdinMessage::ControlRequest {
            request_id: request_id.clone(),
            request,
        }) {
            return Task::ready(Err(error));
        }
        self.pending_requests.insert(request_id, sender);
        cx.background_spawn(
            async move { receiver.await.map_err(|_| anyhow!("Claude Code exited"))? },
        )
    }

    pub fn send_prompt(
        &mut self,
        text: String,
        images: Vec<PromptImage>,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let text = text.trim_end().to_string();
        if text.is_empty() && images.is_empty() {
            return Ok(());
        }
        if let Err(error) = self.start(cx) {
            self.conversation
                .push_notice(NoticeKind::Error, format!("{error:#}"), Utc::now());
            cx.notify();
            return Err(error);
        }
        let mut content = Vec::new();
        for image in &images {
            content.push(ContentBlock::Image {
                source: ImageSource::base64(image.media_type.clone(), image.base64.clone()),
            });
        }
        if !text.is_empty() {
            content.push(ContentBlock::Text { text: text.clone() });
        }
        self.send(&StdinMessage::user(self.session_id(), content))?;
        if self.conversation.turn == crate::conversation::TurnState::Idle {
            self.turn_progressed = false;
        }
        self.conversation
            .push_user_prompt(text, images.len(), Utc::now());
        self.last_activity = Utc::now();
        cx.emit(SessionEvent::Updated);
        cx.notify();
        Ok(())
    }

    pub fn interrupt(&mut self, cx: &mut Context<Self>) {
        if self.process.is_none() {
            return;
        }
        let pending: Vec<String> = self
            .conversation
            .pending_permissions()
            .map(|request| request.request_id.clone())
            .collect();
        for request_id in pending {
            self.respond_permission(
                &request_id,
                PermissionDecision::Deny {
                    message: "The user interrupted.".into(),
                    interrupt: true,
                },
                cx,
            );
        }
        self.request(OutgoingControlRequest::Interrupt, cx).detach();
        cx.notify();
    }

    pub fn respond_permission(
        &mut self,
        request_id: &str,
        decision: PermissionDecision,
        cx: &mut Context<Self>,
    ) {
        let status = match decision {
            PermissionDecision::Allow { .. } => PermissionStatus::Allowed,
            PermissionDecision::Deny { .. } => PermissionStatus::Denied,
        };
        match serde_json::to_value(&decision) {
            Ok(value) => self.respond(request_id.to_string(), Ok(value)),
            Err(error) => self.respond(request_id.to_string(), Err(error.into())),
        }
        self.conversation.set_permission_status(request_id, status);
        cx.emit(SessionEvent::Updated);
        cx.notify();
    }

    pub fn set_permission_mode(&mut self, mode: PermissionMode, cx: &mut Context<Self>) {
        self.conversation.permission_mode = Some(mode);
        if self.process.is_some() {
            self.request(OutgoingControlRequest::SetPermissionMode { mode }, cx)
                .detach();
        }
        cx.emit(SessionEvent::Updated);
        cx.notify();
    }

    pub fn set_model(&mut self, model: Option<String>, cx: &mut Context<Self>) {
        self.model = model.clone();
        if self.process.is_some() {
            self.request(OutgoingControlRequest::SetModel { model }, cx)
                .detach();
        }
        cx.emit(SessionEvent::Updated);
        cx.notify();
    }

    pub fn selected_model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    pub fn set_max_thinking_tokens(&mut self, tokens: Option<u32>, cx: &mut Context<Self>) {
        if self.process.is_some() {
            self.request(
                OutgoingControlRequest::SetMaxThinkingTokens {
                    max_thinking_tokens: tokens,
                },
                cx,
            )
            .detach();
        }
    }

    pub fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(mut process) = self.process.take() {
            process.kill();
        }
        self._events = None;
        self.status = ProcessStatus::NotStarted;
        cx.notify();
    }
}

impl Drop for ClaudeSession {
    fn drop(&mut self) {
        if let Some(mut process) = self.process.take() {
            process.kill();
        }
    }
}

pub fn save_dirty_buffer(project: &Entity<Project>, path: &Path, cx: &mut App) -> Task<Result<()>> {
    let buffer = project.read_with(cx, |project, cx| {
        let project_path = project.find_project_path(path, cx)?;
        project.get_open_buffer(&project_path, cx)
    });
    let Some(buffer) = buffer else {
        return Task::ready(Ok(()));
    };
    if !buffer.read(cx).is_dirty() {
        return Task::ready(Ok(()));
    }
    project.update(cx, |project, cx| project.save_buffer(buffer, cx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::{Entry, TurnState};
    use gpui::{BorrowAppContext as _, TestAppContext};
    use std::cell::RefCell;
    use std::rc::Rc;

    struct FakeLaunch {
        options: LaunchOptions,
        events: Option<mpsc::UnboundedSender<ProcessEvent>>,
        stdin: mpsc::UnboundedReceiver<String>,
    }

    impl FakeLaunch {
        fn send(&self, line: &str) {
            let message = CliMessage::parse(line).expect("valid protocol line");
            self.events
                .as_ref()
                .expect("the process is running")
                .unbounded_send(ProcessEvent::Message(message))
                .expect("the session reads events");
        }

        fn exit(&mut self) {
            if let Some(events) = self.events.take() {
                events.unbounded_send(ProcessEvent::Exited(Some(143))).ok();
            }
        }

        fn prompts(&mut self) -> Vec<String> {
            let mut prompts = Vec::new();
            while let Ok(line) = self.stdin.try_recv() {
                if line.contains(r#""type":"user""#) {
                    prompts.push(line);
                }
            }
            prompts
        }
    }

    fn fake_session(
        cx: &mut TestAppContext,
    ) -> (Entity<ClaudeSession>, Rc<RefCell<Vec<FakeLaunch>>>) {
        cx.update(|cx| {
            let store = settings::SettingsStore::test(cx);
            cx.set_global(store);
            ClaudeCodeSettings::register(cx);
        });
        let launches = Rc::new(RefCell::new(Vec::<FakeLaunch>::new()));
        let session = cx.new(|cx| {
            let mut session =
                ClaudeSession::new(WeakEntity::new_invalid(), PathBuf::from("/p"), None, cx);
            let launches = launches.clone();
            session.set_fake_spawner(Rc::new(move |options| {
                let (process, events_rx, events_tx, stdin) = ClaudeProcess::fake();
                launches.borrow_mut().push(FakeLaunch {
                    options: options.clone(),
                    events: Some(events_tx),
                    stdin,
                });
                (process, events_rx)
            }));
            session
        });
        (session, launches)
    }

    const INIT: &str = r#"{"type":"system","subtype":"init","session_id":"s1","model":"claude-opus-5-5","slash_commands":[]}"#;
    const TOOL_CALL: &str = r#"{"type":"assistant","session_id":"s1","message":{"id":"m1","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"sleep 120"}}]}}"#;

    fn notices(session: &Entity<ClaudeSession>, kind: NoticeKind, cx: &TestAppContext) -> Vec<String> {
        session.read_with(cx, |session, _| {
            session
                .conversation
                .entries
                .iter()
                .filter_map(|entry| match entry {
                    Entry::Notice(notice) if notice.kind == kind => Some(notice.text.clone()),
                    _ => None,
                })
                .collect()
        })
    }

    #[gpui::test]
    async fn a_restarted_process_continues_the_interrupted_turn_once(cx: &mut TestAppContext) {
        let (session, launches) = fake_session(cx);
        session.update(cx, |session, cx| {
            session
                .send_prompt("run sleep 120".into(), Vec::new(), cx)
                .unwrap()
        });
        launches.borrow()[0].send(INIT);
        launches.borrow()[0].send(TOOL_CALL);
        cx.run_until_parked();
        assert_eq!(launches.borrow_mut()[0].prompts().len(), 1);

        // The process is killed during the tool call.
        launches.borrow_mut()[0].exit();
        cx.run_until_parked();
        assert_eq!(launches.borrow().len(), 2, "the process is restarted");
        assert_eq!(launches.borrow()[1].options.resume.as_deref(), Some("s1"));
        let prompts = launches.borrow_mut()[1].prompts();
        assert_eq!(prompts.len(), 1, "{prompts:?}");
        assert!(prompts[0].contains(CONTINUE_PROMPT));
        assert_eq!(notices(&session, NoticeKind::Info, cx), [CONTINUED_NOTICE]);
        assert!(notices(&session, NoticeKind::Error, cx).is_empty());
        session.read_with(cx, |session, _| {
            assert_eq!(session.conversation.turn, TurnState::Running);
            assert_eq!(session.status, ProcessStatus::Running);
        });

        // Killed again before Claude answered: not restarted a second time.
        launches.borrow_mut()[1].exit();
        cx.run_until_parked();
        assert_eq!(launches.borrow().len(), 2);
        assert_eq!(notices(&session, NoticeKind::Info, cx), [CONTINUED_NOTICE]);
        assert_eq!(notices(&session, NoticeKind::Error, cx).len(), 1);
        session.read_with(cx, |session, _| {
            assert_eq!(session.conversation.turn, TurnState::Idle)
        });
    }

    #[gpui::test]
    async fn plan_mode_and_the_setting_keep_an_interrupted_turn_stopped(
        cx: &mut TestAppContext,
    ) {
        let (session, launches) = fake_session(cx);
        session.update(cx, |session, _| {
            session.conversation.permission_mode = Some(PermissionMode::Plan)
        });
        session.update(cx, |session, cx| {
            session.send_prompt("plan it".into(), Vec::new(), cx).unwrap()
        });
        launches.borrow()[0].send(INIT);
        launches.borrow()[0].send(TOOL_CALL);
        cx.run_until_parked();
        launches.borrow_mut()[0].exit();
        cx.run_until_parked();
        assert_eq!(launches.borrow().len(), 1);
        assert_eq!(notices(&session, NoticeKind::Error, cx).len(), 1);

        cx.update(|cx| {
            cx.update_global::<settings::SettingsStore, ()>(|store, cx| {
                store.update_user_settings(cx, |settings| {
                    settings.claude_code.get_or_insert_default().continue_after_reload =
                        Some(false);
                });
            });
        });
        session.update(cx, |session, cx| {
            session.conversation.permission_mode = Some(PermissionMode::Default);
            session.send_prompt("again".into(), Vec::new(), cx).unwrap()
        });
        launches.borrow()[1].send(TOOL_CALL);
        cx.run_until_parked();
        launches.borrow_mut()[1].exit();
        cx.run_until_parked();
        assert_eq!(launches.borrow().len(), 2);
        assert!(notices(&session, NoticeKind::Info, cx).is_empty());
    }
}
