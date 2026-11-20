pub mod conversation;
pub mod diff_review;
pub mod extras;
pub mod file_picker;
pub mod onboarding;
pub mod panel;
pub mod process;
pub mod protocol;
pub mod session;
pub mod sessions;
pub mod sessions_ui;
pub mod settings;
pub mod view;

use ::settings::Settings as _;
use gpui::{App, AppContext as _, Context, Entity, Window, actions};
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
    cx.set_global(claude_ide::GlobalDiffReviewProvider(std::rc::Rc::new(
        diff_review::PanelDiffProvider,
    )));
    sessions::load_store(cx);
    cx.observe_new(|workspace: &mut Workspace, _, cx| {
        register_workspace_actions(workspace);
        cx.subscribe(&cx.entity(), |_, _, event: &workspace::Event, cx| {
            if let workspace::Event::ItemRemoved { item_id } = event {
                let item_id = *item_id;
                // The tab records itself as closing while it is removed;
                // look once that has happened.
                cx.defer(move |cx| sessions::note_item_removed(item_id, cx));
            }
        })
        .detach();
    })
    .detach();
}

fn register_workspace_actions(workspace: &mut Workspace) {
    workspace
        .register_action(|workspace, _: &ToggleFocus, window, cx| {
            workspace.toggle_panel_focus::<ClaudeCodePanel>(window, cx);
        })
        .register_action(|workspace, _: &Open, window, cx| {
            if ClaudeCodeSettings::get_global(cx).use_terminal {
                extras::open_in_terminal(workspace, window, cx);
            } else {
                open_preferred(workspace, window, cx);
            }
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
        .register_action(|workspace, _: &AddToChat, window, cx| {
            extras::add_active_file_to_chat(workspace, window, cx);
        })
        .register_action(
            |workspace, action: &zed_actions::claude_code::AddPathsToChat, window, cx| {
                extras::add_paths_to_chat(workspace, action.paths.clone(), window, cx);
            },
        )
        .register_action(|workspace, _: &Login, window, cx| {
            extras::login(workspace, window, cx);
        })
        .register_action(|workspace, _: &Logout, _, cx| {
            extras::logout(workspace, cx);
        })
        .register_action(|workspace, _: &ShowLogs, window, cx| {
            extras::show_logs(workspace, window, cx);
        })
        .register_action(|workspace, _: &OpenInTerminal, window, cx| {
            extras::open_in_terminal(workspace, window, cx);
        })
        .register_action(|workspace, _: &InstallPlugin, window, cx| {
            extras::install_plugin(workspace, window, cx);
        })
        .register_action(|workspace, _: &OpenWalkthrough, window, cx| {
            extras::open_walkthrough(workspace, window, cx);
        })
        .register_action(|workspace, _: &ToggleDictation, _, cx| {
            extras::toggle_dictation(workspace, cx);
        })
        .register_action(|workspace, _: &NewConversation, window, cx| {
            if let Some(panel) = workspace.focus_panel::<ClaudeCodePanel>(window, cx) {
                panel.update(cx, |panel, cx| panel.new_conversation(window, cx));
            }
        })
        .register_action(show_sessions)
        .register_action(reopen_closed_session)
        .register_action(rename_session)
        .register_action(mark_session_unread)
        .register_action(add_session_to_group)
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

/// Handles `zed://claude-code/open?prompt=…&session=…`: resumes the session
/// when one is given, and fills the prompt in without sending it.
pub fn open_from_uri(
    workspace: &mut Workspace,
    prompt: Option<String>,
    session: Option<String>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let view = match session {
        Some(session_id) => open(workspace, Location::NewTab, Some(session_id), window, cx),
        None => open_preferred(workspace, window, cx),
    };
    if let (Some(view), Some(prompt)) = (view, prompt) {
        view.update(cx, |view, cx| view.set_input_text(&prompt, window, cx));
    }
}

fn open_session_ids(workspace: &Workspace, cx: &App) -> Vec<String> {
    let mut ids: Vec<String> = workspace
        .items_of_type::<ClaudeCodeTab>(cx)
        .filter_map(|tab| {
            tab.read(cx)
                .view
                .read(cx)
                .session
                .read(cx)
                .session_id()
                .map(str::to_string)
        })
        .collect();
    if let Some(panel) = workspace.panel::<ClaudeCodePanel>(cx)
        && let Some(id) = panel.read(cx).view.read(cx).session.read(cx).session_id()
    {
        ids.push(id.to_string());
    }
    ids
}

fn show_sessions(
    workspace: &mut Workspace,
    _: &ShowSessions,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let cwd = panel::workspace_cwd(workspace.project(), cx);
    let dir = sessions::transcripts_dir(&claude_ide::claude_config_dir(), &cwd);
    let store = sessions::read_store(cx);
    let days = ClaudeCodeSettings::get_global(cx).archive_inactive_sessions;
    let open_ids = open_session_ids(workspace, cx);
    let in_tab = workspace.active_item_as::<ClaudeCodeTab>(cx).is_some();
    let listing = cx.background_spawn(async move { sessions::list_sessions(&dir, &store) });
    cx.spawn_in(window, async move |workspace, cx| {
        let mut sessions = listing.await;
        let to_archive =
            sessions::sessions_to_archive(&sessions, days, &open_ids, chrono::Utc::now());
        workspace
            .update_in(cx, |workspace, window, cx| {
                if !to_archive.is_empty() {
                    sessions::update_store(cx, |store| {
                        for session_id in &to_archive {
                            store.meta_mut(session_id).archived = true;
                        }
                    });
                    for session in &mut sessions {
                        if to_archive.contains(&session.session_id) {
                            session.meta.archived = true;
                        }
                    }
                }
                let weak_workspace = cx.entity().downgrade();
                let on_open: sessions_ui::OnOpen =
                    std::sync::Arc::new(move |workspace, session_id, window, cx| {
                        let location = if in_tab {
                            Location::NewTab
                        } else {
                            Location::Panel
                        };
                        open(workspace, location, Some(session_id), window, cx);
                    });
                workspace.toggle_modal(window, cx, |window, cx| {
                    let delegate =
                        sessions_ui::SessionsDelegate::new(weak_workspace, sessions, on_open);
                    picker::Picker::uniform_list(delegate, window, cx)
                });
            })
            .ok();
    })
    .detach();
}

fn reopen_closed_session(
    workspace: &mut Workspace,
    _: &ReopenClosedSession,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    if ClaudeCodeSettings::get_global(cx).enable_reopen_closed_session_shortcut
        && sessions::last_closed_was_session(cx)
        && let Some(session_id) = sessions::take_last_closed(cx)
    {
        open(workspace, Location::NewTab, Some(session_id), window, cx);
        return;
    }
    window.dispatch_action(Box::new(workspace::pane::ReopenClosedItem), cx);
}

fn active_session_id(
    workspace: &Workspace,
    cx: &App,
) -> Option<(Entity<view::ClaudeCodeView>, String)> {
    let view = active_view(workspace, cx)?;
    let session_id = view.read(cx).session.read(cx).session_id()?.to_string();
    Some((view, session_id))
}

fn rename_session(
    workspace: &mut Workspace,
    _: &RenameSession,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let Some((view, session_id)) = active_session_id(workspace, cx) else {
        return;
    };
    let current = view.read(cx).title(cx).to_string();
    workspace.toggle_modal(window, cx, |window, cx| {
        sessions_ui::TextPrompt::new(
            "Rename session",
            &current,
            move |title, cx| {
                let title = (!title.is_empty()).then_some(title);
                sessions::update_store(cx, |store| {
                    store.meta_mut(&session_id).title = title.clone()
                });
                view.update(cx, |_, cx| cx.emit(view::ViewEvent::TitleChanged));
            },
            window,
            cx,
        )
    });
}

fn mark_session_unread(
    workspace: &mut Workspace,
    _: &MarkSessionUnread,
    _: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let Some((_, session_id)) = active_session_id(workspace, cx) else {
        return;
    };
    sessions::update_store(cx, |store| store.meta_mut(&session_id).unread = true);
    if let Some(tab) = workspace.active_item_as::<ClaudeCodeTab>(cx) {
        tab.update(cx, |tab, cx| {
            tab.unread = true;
            cx.emit(panel::TabEvent::UpdateTab);
        });
    }
}

fn add_session_to_group(
    workspace: &mut Workspace,
    _: &AddSessionToGroup,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let Some((_, session_id)) = active_session_id(workspace, cx) else {
        return;
    };
    let current = sessions::read_store(cx)
        .sessions
        .get(&session_id)
        .and_then(|meta| meta.group.clone())
        .unwrap_or_default();
    workspace.toggle_modal(window, cx, |window, cx| {
        sessions_ui::TextPrompt::new(
            "Add session to group (empty removes it from its group)",
            &current,
            move |group, cx| {
                let group = (!group.is_empty()).then_some(group);
                sessions::update_store(cx, |store| {
                    store.meta_mut(&session_id).group = group.clone()
                });
            },
            window,
            cx,
        )
    });
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
    if ClaudeCodeSettings::get_global(cx).use_terminal {
        // Terminal mode: mention the selection in the connected `claude`.
        let workspace_entity = cx.entity();
        if let Some(ide) = claude_ide::ClaudeIde::for_workspace(&workspace_entity, cx) {
            ide.read(cx).send_at_mention(&selection.file_path, rows);
        }
        return;
    }
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
