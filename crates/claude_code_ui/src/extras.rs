use std::path::PathBuf;

use gpui::{App, AppContext as _, Context, Window};
use language::Buffer;
use settings::Settings as _;
use task::{RevealStrategy, SpawnInTerminal};
use workspace::notifications::NotificationId;
use workspace::{Toast, Workspace};

use crate::panel::{active_view, open_preferred, workspace_cwd};
use crate::process::find_claude;
use crate::session::ClaudeLogs;
use crate::settings::ClaudeCodeSettings;
use crate::view::Attachment;

struct ClaudeCodeToast;

fn toast(workspace: &mut Workspace, message: impl Into<String>, cx: &mut Context<Workspace>) {
    workspace.show_toast(
        Toast::new(NotificationId::unique::<ClaudeCodeToast>(), message.into()).autohide(),
        cx,
    );
}

fn claude_program(cx: &App) -> Option<String> {
    let settings = ClaudeCodeSettings::get_global(cx);
    if let Some(wrapper) = &settings.claude_process_wrapper {
        return Some(wrapper.clone());
    }
    find_claude(Some(paths::home_dir().as_path()), std::env::var_os("PATH"))
        .map(|path| path.to_string_lossy().into_owned())
}

/// Runs `claude` with `args` in a terminal of this window. The terminal gets
/// the IDE server's port, so the CLI connects to Vireo by itself.
pub fn run_in_terminal(
    workspace: &mut Workspace,
    label: &str,
    args: Vec<String>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let Some(program) = claude_program(cx) else {
        toast(
            workspace,
            "Claude Code is not installed. Install it from https://claude.com/claude-code.",
            cx,
        );
        return;
    };
    let project = workspace.project().clone();
    let cwd = workspace_cwd(&project, cx);
    let mut env: collections::HashMap<String, String> = ClaudeCodeSettings::get_global(cx)
        .environment_variables
        .iter()
        .cloned()
        .collect();
    env.extend(project.update(cx, |project, cx| project.extra_terminal_env(cx)));
    let command_label = std::iter::once(program.clone())
        .chain(args.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ");
    let spawn = SpawnInTerminal {
        label: label.to_string(),
        full_label: label.to_string(),
        command: Some(program),
        args,
        command_label,
        cwd: Some(cwd),
        env,
        use_new_terminal: true,
        allow_concurrent_runs: true,
        reveal: RevealStrategy::Always,
        ..Default::default()
    };
    workspace.spawn_in_terminal(spawn, window, cx).detach();
}

pub fn open_in_terminal(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    run_in_terminal(workspace, "Claude Code", Vec::new(), window, cx);
}

pub fn login(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    run_in_terminal(
        workspace,
        "Claude Code: Log In",
        vec!["auth".into(), "login".into()],
        window,
        cx,
    );
}

pub fn install_plugin(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    run_in_terminal(
        workspace,
        "Claude Code: Plugins",
        vec!["/plugin".into()],
        window,
        cx,
    );
}

pub fn logout(workspace: &mut Workspace, cx: &mut Context<Workspace>) {
    let Some(program) = claude_program(cx) else {
        toast(workspace, "Claude Code is not installed.", cx);
        return;
    };
    let task = cx.background_spawn(async move {
        util::command::new_command(&program)
            .args(["auth", "logout"])
            .output()
            .await
    });
    cx.spawn(async move |workspace, cx| {
        let message = match task.await {
            Ok(output) if output.status.success() => "Logged out of Claude Code.".to_string(),
            Ok(output) => format!(
                "Logout failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
            Err(error) => format!("Logout failed: {error}"),
        };
        workspace
            .update(cx, |workspace, cx| toast(workspace, message, cx))
            .ok();
    })
    .detach();
}

/// Opens the panel's log (Vireo's notes and the CLI's debug output) as a
/// read-only buffer.
pub fn show_logs(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let text = ClaudeLogs::text(cx);
    let text = if text.is_empty() {
        "No Claude Code output yet.".to_string()
    } else {
        text
    };
    let project = workspace.project().clone();
    let buffer = cx.new(|cx| Buffer::local(text, cx));
    let editor = cx.new(|cx| {
        let mut editor = editor::Editor::for_buffer(buffer, Some(project), window, cx);
        editor.set_read_only(true);
        editor
    });
    workspace.add_item_to_active_pane(Box::new(editor), None, true, window, cx);
}

pub fn add_paths_to_chat(
    workspace: &mut Workspace,
    paths: Vec<PathBuf>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    if ClaudeCodeSettings::get_global(cx).use_terminal {
        let workspace_entity = cx.entity();
        if let Some(ide) = claude_ide::ClaudeIde::for_workspace(&workspace_entity, cx) {
            for path in &paths {
                ide.read(cx).send_at_mention(path, None);
            }
        }
        return;
    }
    let view = active_view(workspace, cx).or_else(|| open_preferred(workspace, window, cx));
    if let Some(view) = view {
        view.update(cx, |view, cx| {
            for path in paths {
                view.add_attachment(Attachment::Mention(path), cx);
            }
        });
    }
}

pub fn add_active_file_to_chat(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let path = workspace
        .active_item(cx)
        .and_then(|item| item.project_path(cx))
        .and_then(|project_path| {
            workspace
                .project()
                .read(cx)
                .absolute_path(&project_path, cx)
        });
    if let Some(path) = path {
        add_paths_to_chat(workspace, vec![path], window, cx);
    }
}

/// "Open Walkthrough": shows the "Get started" checklist again and opens
/// Claude where the checklist lives, a new conversation.
pub fn open_walkthrough(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    if ClaudeCodeSettings::get_global(cx).hide_onboarding {
        let fs = <dyn fs::Fs>::global(cx);
        settings::update_settings_file(fs, cx, |settings, _| {
            settings.claude_code.get_or_insert_default().hide_onboarding = Some(false);
        });
    }
    let has_conversation = active_view(workspace, cx).is_some_and(|view| {
        !view
            .read(cx)
            .session
            .read(cx)
            .conversation
            .entries
            .is_empty()
    });
    if has_conversation {
        crate::panel::open(workspace, crate::panel::Location::NewTab, None, window, cx);
    } else {
        open_preferred(workspace, window, cx);
    }
}

pub fn toggle_dictation(workspace: &mut Workspace, cx: &mut Context<Workspace>) {
    toast(
        workspace,
        "Voice dictation is not available in Vireo yet. Type your prompt, or use your system's dictation.",
        cx,
    );
}
