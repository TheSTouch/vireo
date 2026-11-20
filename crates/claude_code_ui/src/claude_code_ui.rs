pub mod conversation;
pub mod file_picker;
pub mod onboarding;
pub mod panel;
pub mod process;
pub mod protocol;
pub mod session;
pub mod sessions;
pub mod settings;
pub mod view;

use ::settings::Settings as _;
use gpui::{App, Context, Window, actions};
use workspace::Workspace;

pub use crate::panel::{ClaudeCodePanel, ClaudeCodeTab};
use crate::panel::{Location, active_view, open, open_in_new_window, open_preferred};
pub use crate::settings::ClaudeCodeSettings;

actions!(
    claude_code,
    [
        /// Opens Claude Code where it was opened last.
        Open,
        /// Opens a new Claude Code conversation in a new editor tab.
        OpenInNewTab,
        /// Opens Claude Code in the primary editor group.
        OpenInPrimaryEditor,
        /// Opens Claude Code in a new window.
        OpenInNewWindow,
        /// Opens Claude Code in the side bar.
        OpenInSideBar,
        /// Toggles focus on the Claude Code side bar panel.
        ToggleFocus,
        /// Starts a new conversation in the focused Claude Code view.
        NewConversation,
        /// Reopens the most recently closed Claude Code session.
        ReopenClosedSession,
        /// Moves focus to the Claude Code message box.
        FocusInput,
        /// Moves focus from Claude Code back to the editor.
        BlurInput,
        /// Moves focus to the last message of the conversation.
        FocusLastMessage,
        /// Sends the message in the message box.
        Send,
        /// Stops the turn Claude is working on.
        Interrupt,
        /// Puts the suggested prompt in the message box.
        AcceptSuggestion,
        /// Hides or shows tool calls and other activity.
        ToggleFocusView,
        /// Turns extended thinking on or off.
        ToggleThinking,
        /// Cycles through the permission modes.
        CycleMode,
        /// Allows the pending tool use once.
        AllowPermission,
        /// Allows the pending tool use and similar ones from now on.
        AllowPermissionAlways,
        /// Denies the pending tool use.
        DenyPermission,
        /// Accepts the whole proposed diff.
        AcceptProposedDiff,
        /// Rejects the whole proposed diff.
        RejectProposedDiff,
        /// Accepts the proposed change at the cursor.
        AcceptProposedHunk,
        /// Rejects the proposed change at the cursor.
        RejectProposedHunk,
        /// Inserts an @-mention of the editor selection into the message box.
        InsertAtMention,
        /// Sends an @-mention of the editor selection to a `claude` connected from a terminal.
        InsertAtMentioned,
        /// Adds the active file to the conversation.
        AddToChat,
        /// Shows past conversations.
        ShowSessions,
        /// Marks the active session as unread.
        MarkSessionUnread,
        /// Renames the active session.
        RenameSession,
        /// Adds the active session to a group.
        AddSessionToGroup,
        /// Logs out of Claude Code.
        Logout,
        /// Logs in to Claude Code.
        Login,
        /// Shows Claude Code's logs.
        ShowLogs,
        /// Runs `claude` in a terminal.
        OpenInTerminal,
        /// Installs a Claude Code plugin.
        InstallPlugin,
        /// Opens the Claude Code walkthrough.
        OpenWalkthrough,
        /// Starts or stops voice dictation.
        ToggleDictation,
    ]
);

pub fn init(cx: &mut App) {
    ClaudeCodeSettings::register(cx);
    claude_ide::init(cx);
    cx.observe_new(|workspace: &mut Workspace, _, _| register_workspace_actions(workspace))
        .detach();
}

fn register_workspace_actions(workspace: &mut Workspace) {
    workspace
        .register_action(|workspace, _: &ToggleFocus, window, cx| {
            workspace.toggle_panel_focus::<ClaudeCodePanel>(window, cx);
        })
        .register_action(|workspace, _: &Open, window, cx| {
            open_preferred(workspace, window, cx);
        })
        .register_action(|workspace, _: &OpenInNewTab, window, cx| {
            open(workspace, Location::NewTab, None, window, cx);
        })
        .register_action(|workspace, _: &OpenInPrimaryEditor, window, cx| {
            open(workspace, Location::PrimaryEditor, None, window, cx);
        })
        .register_action(|workspace, _: &OpenInSideBar, window, cx| {
            open(workspace, Location::Panel, None, window, cx);
        })
        .register_action(|workspace, _: &OpenInNewWindow, _, cx| {
            open_in_new_window(workspace, cx);
        })
        .register_action(|workspace, _: &NewConversation, window, cx| {
            if let Some(panel) = workspace.focus_panel::<ClaudeCodePanel>(window, cx) {
                panel.update(cx, |panel, cx| panel.new_conversation(window, cx));
            }
        })
        .register_action(focus_input)
        .register_action(insert_at_mention)
        .register_action(insert_at_mentioned);
}

fn focus_input(
    workspace: &mut Workspace,
    _: &FocusInput,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    if ClaudeCodeSettings::get_global(cx).use_terminal {
        window.dispatch_action(Box::new(OpenInTerminal), cx);
        return;
    }
    if workspace.active_item_as::<ClaudeCodeTab>(cx).is_some()
        && let Some(view) = active_view(workspace, cx)
    {
        window.focus(&view.read(cx).input_focus_handle(cx), cx);
        return;
    }
    open_preferred(workspace, window, cx);
}

fn editor_selection(workspace: &Workspace, cx: &App) -> Option<claude_ide::SelectionInfo> {
    let editor = workspace.active_item(cx)?.act_as::<editor::Editor>(cx)?;
    claude_ide::editor_selection(editor.read(cx), cx)
}

fn insert_at_mention(
    workspace: &mut Workspace,
    _: &InsertAtMention,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let Some(selection) = editor_selection(workspace, cx) else {
        return;
    };
    let rows = (!selection.is_empty()).then_some(selection.rows);
    let view = active_view(workspace, cx).or_else(|| open_preferred(workspace, window, cx));
    if let Some(view) = view {
        view.update(cx, |view, cx| {
            view.insert_mention(&selection.file_path, rows, window, cx)
        });
    }
}

fn insert_at_mentioned(
    workspace: &mut Workspace,
    _: &InsertAtMentioned,
    _: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let Some(selection) = editor_selection(workspace, cx) else {
        return;
    };
    let rows = (!selection.is_empty()).then_some(selection.rows);
    let workspace_entity = cx.entity();
    if let Some(ide) = claude_ide::ClaudeIde::for_workspace(&workspace_entity, cx) {
        ide.read(cx).send_at_mention(&selection.file_path, rows);
    }
}
