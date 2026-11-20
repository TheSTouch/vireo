use std::path::PathBuf;

use anyhow::Result;
use gpui::{
    Action, App, AsyncWindowContext, Context, Entity, EventEmitter, FocusHandle, Focusable, Pixels,
    SharedString, Subscription, Task, WeakEntity, Window, prelude::*, px,
};
use project::Project;
use settings::{ClaudeCodePreferredLocation, Settings as _};
use ui::prelude::*;
use workspace::dock::{DockPosition, Panel, PanelEvent};
use workspace::item::{Item, ItemEvent};
use workspace::{OpenOptions, Workspace};

use crate::session::ClaudeSession;
use crate::settings::ClaudeCodeSettings;
use crate::view::{ClaudeCodeView, ViewEvent};
use crate::{NewConversation, ToggleFocus};

const PANEL_KEY: &str = "ClaudeCodePanel";
const CONTINUE_PROMPT: &str = "Continue from where you left off.";

/// The directory Claude runs in: the first folder of the workspace.
pub fn workspace_cwd(project: &Entity<Project>, cx: &App) -> PathBuf {
    project
        .read(cx)
        .visible_worktrees(cx)
        .next()
        .map(|worktree| worktree.read(cx).abs_path().to_path_buf())
        .unwrap_or_else(|| paths::home_dir().clone())
}

/// Takes the project separately because callers are often inside an update of
/// the workspace, which cannot be read then.
pub fn new_session(
    workspace: &Entity<Workspace>,
    project: Entity<Project>,
    resume: Option<String>,
    cx: &mut App,
) -> Entity<ClaudeSession> {
    let cwd = workspace_cwd(&project, cx);
    let ide = claude_ide::ClaudeIde::for_workspace(workspace, cx).map(|ide| ide.downgrade());
    let transcript = resume.as_ref().map(|session_id| {
        crate::sessions::transcripts_dir(&claude_ide::claude_config_dir(), &cwd)
            .join(format!("{session_id}.jsonl"))
    });
    cx.new(|cx| {
        let mut session = ClaudeSession::new(project.downgrade(), cwd, resume, cx);
        session.set_ide(ide);
        if let Some(path) = transcript {
            match crate::sessions::load_transcript(&path) {
                Ok(mut conversation) => {
                    conversation.permission_mode = session.conversation.permission_mode;
                    if conversation.session_id.is_none() {
                        conversation.session_id = session.conversation.session_id.clone();
                    }
                    session.conversation = conversation;
                }
                Err(error) => log::warn!("failed to load the Claude session: {error:#}"),
            }
        }
        session
    })
}

pub fn new_view(
    workspace: &Entity<Workspace>,
    project: Entity<Project>,
    resume: Option<String>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<ClaudeCodeView> {
    let session = new_session(workspace, project.clone(), resume, cx);
    let weak_workspace = workspace.downgrade();
    cx.new(|cx| ClaudeCodeView::new(session, weak_workspace, project, window, cx))
}

/// Claude Code in the right dock, which Vireo shows as the secondary side bar.
pub struct ClaudeCodePanel {
    workspace: WeakEntity<Workspace>,
    pub view: Entity<ClaudeCodeView>,
    focus_handle: FocusHandle,
    position: DockPosition,
    cwd_key: String,
    saved_state: Option<crate::sessions::OpenSession>,
    _view_subscription: Subscription,
}

impl ClaudeCodePanel {
    pub fn load(
        workspace: WeakEntity<Workspace>,
        cx: AsyncWindowContext,
    ) -> Task<Result<Entity<Self>>> {
        cx.spawn(async move |cx| {
            workspace.update_in(cx, |workspace, window, cx| {
                let project = workspace.project().clone();
                let workspace_entity = cx.entity();
                cx.new(|cx| Self::new(workspace_entity, project, window, cx))
            })
        })
    }

    fn new(
        workspace: Entity<Workspace>,
        project: Entity<Project>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let cwd_key = workspace_cwd(&project, cx).to_string_lossy().into_owned();
        // Bring back the session the panel had before Vireo restarted.
        let saved = crate::sessions::read_store(cx)
            .open
            .get(&cwd_key)
            .and_then(|open| open.iter().find(|session| !session.in_tab).cloned());
        let view = new_view(
            &workspace,
            project,
            saved.as_ref().map(|saved| saved.session_id.clone()),
            window,
            cx,
        );
        if let Some(saved) = &saved
            && saved.interrupted
            && !saved.plan_mode
            && ClaudeCodeSettings::get_global(cx).continue_after_reload
        {
            let session = view.read(cx).session.clone();
            session.update(cx, |session, cx| {
                if let Err(error) = session.send_prompt(CONTINUE_PROMPT.to_string(), Vec::new(), cx)
                {
                    log::warn!("failed to continue the Claude session: {error:#}");
                }
            });
        }
        let subscription = Self::subscribe_view(&view, cx);
        Self {
            workspace: workspace.downgrade(),
            view,
            focus_handle: cx.focus_handle(),
            position: DockPosition::Right,
            cwd_key,
            saved_state: saved,
            _view_subscription: subscription,
        }
    }

    fn subscribe_view(view: &Entity<ClaudeCodeView>, cx: &mut Context<Self>) -> Subscription {
        cx.subscribe(view, |this, _, _: &ViewEvent, cx| {
            this.save_state(cx);
            cx.notify();
        })
    }

    /// Remembers the panel's session and whether a turn is running, for
    /// continue_after_reload. Written only when that changes.
    fn save_state(&mut self, cx: &mut Context<Self>) {
        let session = self.view.read(cx).session.read(cx);
        let Some(session_id) = session.session_id().map(str::to_string) else {
            return;
        };
        let state = crate::sessions::OpenSession {
            session_id,
            interrupted: session.conversation.turn != crate::conversation::TurnState::Idle,
            plan_mode: session.conversation.permission_mode
                == Some(crate::protocol::PermissionMode::Plan),
            in_tab: false,
        };
        if self.saved_state.as_ref() == Some(&state) {
            return;
        }
        self.saved_state = Some(state.clone());
        let cwd_key = self.cwd_key.clone();
        crate::sessions::update_store(cx, |store| {
            store.open.insert(cwd_key, vec![state]);
        });
    }

    pub fn set_view(&mut self, view: Entity<ClaudeCodeView>, cx: &mut Context<Self>) {
        self._view_subscription = Self::subscribe_view(&view, cx);
        self.view = view;
        cx.notify();
    }

    pub fn new_conversation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        if self
            .view
            .read(cx)
            .session
            .read(cx)
            .conversation
            .entries
            .is_empty()
        {
            window.focus(&self.view.read(cx).input_focus_handle(cx), cx);
            return;
        }
        let old_session = self.view.read(cx).session.clone();
        crate::sessions::remember_closed(&old_session, cx);
        let project = workspace.read(cx).project().clone();
        let view = new_view(&workspace, project, None, window, cx);
        self.set_view(view, cx);
        window.focus(&self.view.read(cx).input_focus_handle(cx), cx);
    }
}

impl EventEmitter<PanelEvent> for ClaudeCodePanel {}

impl Focusable for ClaudeCodePanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.view.focus_handle(cx)
    }
}

impl Render for ClaudeCodePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &NewConversation, window, cx| {
                this.new_conversation(window, cx)
            }))
            .child(self.view.clone())
    }
}

impl Panel for ClaudeCodePanel {
    fn persistent_name() -> &'static str {
        PANEL_KEY
    }

    fn panel_key() -> &'static str {
        PANEL_KEY
    }

    fn activation_focus_handle(&self, cx: &App) -> FocusHandle {
        self.view.read(cx).input_focus_handle(cx)
    }

    fn position(&self, _: &Window, _: &App) -> DockPosition {
        self.position
    }

    fn position_is_valid(&self, _: DockPosition) -> bool {
        true
    }

    fn set_position(&mut self, position: DockPosition, _: &mut Window, cx: &mut Context<Self>) {
        self.position = position;
        cx.notify();
    }

    fn default_size(&self, _: &Window, _: &App) -> Pixels {
        px(420.)
    }

    fn icon(&self, _: &Window, _: &App) -> Option<IconName> {
        Some(IconName::Sparkle)
    }

    fn icon_tooltip(&self, _: &Window, _: &App) -> Option<&'static str> {
        Some("Claude Code")
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        4
    }
}

/// Claude Code as an editor tab, for "Open in New Tab", "Open in Primary
/// Editor" and "Open in New Window".
pub struct ClaudeCodeTab {
    workspace: WeakEntity<Workspace>,
    pub view: Entity<ClaudeCodeView>,
    pub unread: bool,
    pub custom_title: Option<SharedString>,
    _view_subscription: Subscription,
}

#[derive(Clone, Debug)]
pub enum TabEvent {
    UpdateTab,
}

impl ClaudeCodeTab {
    pub fn new(
        workspace: WeakEntity<Workspace>,
        view: Entity<ClaudeCodeView>,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscription = Self::subscribe_view(&view, cx);
        Self {
            workspace,
            view,
            unread: false,
            custom_title: None,
            _view_subscription: subscription,
        }
    }

    fn subscribe_view(view: &Entity<ClaudeCodeView>, cx: &mut Context<Self>) -> Subscription {
        cx.subscribe(view, |this, _, event: &ViewEvent, cx| {
            if *event == ViewEvent::Unread {
                this.unread = true;
            }
            cx.emit(TabEvent::UpdateTab);
            cx.notify();
        })
    }

    fn new_conversation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let old_session = self.view.read(cx).session.clone();
        crate::sessions::remember_closed(&old_session, cx);
        let project = workspace.read(cx).project().clone();
        let view = new_view(&workspace, project, None, window, cx);
        self._view_subscription = Self::subscribe_view(&view, cx);
        self.view = view;
        self.custom_title = None;
        cx.emit(TabEvent::UpdateTab);
        window.focus(&self.view.read(cx).input_focus_handle(cx), cx);
    }
}

impl EventEmitter<TabEvent> for ClaudeCodeTab {}

impl Focusable for ClaudeCodeTab {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.view.read(cx).input_focus_handle(cx)
    }
}

impl Render for ClaudeCodeTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.unread && self.view.focus_handle(cx).contains_focused(window, cx) {
            self.unread = false;
            cx.emit(TabEvent::UpdateTab);
        }
        div()
            .size_full()
            .on_action(cx.listener(|this, _: &NewConversation, window, cx| {
                this.new_conversation(window, cx)
            }))
            .child(self.view.clone())
    }
}

impl Item for ClaudeCodeTab {
    type Event = TabEvent;

    fn tab_extra_context_menu_actions(
        &self,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Vec<(SharedString, Box<dyn gpui::Action>)> {
        vec![
            (
                "Mark Session as Unread".into(),
                Box::new(crate::MarkSessionUnread),
            ),
            ("Rename Session Tab".into(), Box::new(crate::RenameSession)),
            (
                "Add Session Tab to Group".into(),
                Box::new(crate::AddSessionToGroup),
            ),
        ]
    }

    fn tab_content_text(&self, _: usize, cx: &App) -> SharedString {
        self.custom_title
            .clone()
            .unwrap_or_else(|| self.view.read(cx).title(cx))
    }

    fn tab_icon(&self, _: &Window, cx: &App) -> Option<Icon> {
        let running = self.view.read(cx).is_running(cx);
        Some(
            Icon::new(if self.unread {
                IconName::Circle
            } else {
                IconName::Sparkle
            })
            .color(if self.unread || running {
                Color::Accent
            } else {
                Color::Muted
            }),
        )
    }

    fn tab_tooltip_text(&self, _: &App) -> Option<SharedString> {
        Some("Claude Code".into())
    }

    fn to_item_events(event: &Self::Event, f: &mut dyn FnMut(ItemEvent)) {
        match event {
            TabEvent::UpdateTab => f(ItemEvent::UpdateTab),
        }
    }

    fn on_removed(&self, cx: &mut Context<Self>) {
        let session = self.view.read(cx).session.clone();
        crate::sessions::remember_closed(&session, cx);
        crate::sessions::note_claude_tab_closed(cx.entity_id(), cx);
    }
}

pub enum Location {
    Panel,
    NewTab,
    PrimaryEditor,
}

/// Opens a Claude view and focuses its message box. `resume` opens a past
/// session instead of a new one.
pub fn open(
    workspace: &mut Workspace,
    location: Location,
    resume: Option<String>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> Option<Entity<ClaudeCodeView>> {
    match location {
        Location::Panel => {
            let panel = workspace.focus_panel::<ClaudeCodePanel>(window, cx)?;
            if let Some(session_id) = resume {
                let workspace_entity = cx.entity();
                let project = workspace.project().clone();
                let view = new_view(&workspace_entity, project, Some(session_id), window, cx);
                panel.update(cx, |panel, cx| panel.set_view(view, cx));
            }
            let view = panel.read(cx).view.clone();
            window.focus(&view.read(cx).input_focus_handle(cx), cx);
            Some(view)
        }
        Location::NewTab | Location::PrimaryEditor => {
            let workspace_entity = cx.entity();
            let project = workspace.project().clone();
            let view = new_view(&workspace_entity, project, resume, window, cx);
            let tab =
                cx.new(|cx| ClaudeCodeTab::new(workspace_entity.downgrade(), view.clone(), cx));
            if matches!(location, Location::PrimaryEditor) {
                workspace.add_item_to_center(Box::new(tab), window, cx);
            } else {
                workspace.add_item_to_active_pane(Box::new(tab), None, true, window, cx);
            }
            window.focus(&view.read(cx).input_focus_handle(cx), cx);
            Some(view)
        }
    }
}

/// Opens a fork of a conversation in a new tab, the way the extension's
/// "Fork conversation from here" does: the session resumed up to the message
/// before the chosen prompt, under a new session id, with the prompt back in
/// the input to edit. With no earlier message it is a new conversation.
pub fn open_fork(
    workspace: &mut Workspace,
    session_id: String,
    resume_at: Option<String>,
    entries: Vec<crate::conversation::Entry>,
    prompt: String,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> Entity<ClaudeCodeView> {
    let workspace_entity = cx.entity();
    let project = workspace.project().clone();
    let cwd = workspace_cwd(&project, cx);
    let ide =
        claude_ide::ClaudeIde::for_workspace(&workspace_entity, cx).map(|ide| ide.downgrade());
    let session = cx.new(|cx| {
        let resume = resume_at.as_ref().map(|_| session_id);
        let forked = resume.is_some();
        let mut session = ClaudeSession::new(project.downgrade(), cwd, resume, cx);
        session.set_ide(ide);
        if forked {
            session.set_fork_on_start(true);
            session.set_resume_session_at(resume_at);
            // The fork gets its own id from the CLI; until then it has none.
            session.conversation.session_id = None;
            session.conversation.entries = entries;
        }
        session
    });
    let view = cx
        .new(|cx| ClaudeCodeView::new(session, workspace_entity.downgrade(), project, window, cx));
    view.update(cx, |view, cx| view.set_input_text(&prompt, window, cx));
    let tab = cx.new(|cx| ClaudeCodeTab::new(workspace_entity.downgrade(), view.clone(), cx));
    workspace.add_item_to_active_pane(Box::new(tab), None, true, window, cx);
    window.focus(&view.read(cx).input_focus_handle(cx), cx);
    view
}

/// The Claude view to act on in this window: the active Claude tab, or the
/// panel's view.
pub fn active_view(workspace: &Workspace, cx: &App) -> Option<Entity<ClaudeCodeView>> {
    if let Some(tab) = workspace.active_item_as::<ClaudeCodeTab>(cx) {
        return Some(tab.read(cx).view.clone());
    }
    let panel = workspace.panel::<ClaudeCodePanel>(cx)?;
    Some(panel.read(cx).view.clone())
}

pub fn open_preferred(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> Option<Entity<ClaudeCodeView>> {
    match ClaudeCodeSettings::get_global(cx).preferred_location {
        ClaudeCodePreferredLocation::Sidebar => open(workspace, Location::Panel, None, window, cx),
        ClaudeCodePreferredLocation::Panel => {
            if let Some(tab) = workspace.items_of_type::<ClaudeCodeTab>(cx).last() {
                let view = tab.read(cx).view.clone();
                workspace.activate_item(&tab, true, true, window, cx);
                window.focus(&view.read(cx).input_focus_handle(cx), cx);
                return Some(view);
            }
            open(workspace, Location::NewTab, None, window, cx)
        }
    }
}

/// Opens a new window on the same folders with Claude in an editor tab.
pub fn open_in_new_window(workspace: &Workspace, cx: &mut App) {
    let paths: Vec<PathBuf> = workspace
        .visible_worktrees(cx)
        .map(|worktree| worktree.read(cx).abs_path().to_path_buf())
        .collect();
    let app_state = workspace.app_state().clone();
    let open_task = workspace::open_paths(
        &paths,
        app_state,
        OpenOptions {
            open_mode: workspace::OpenMode::NewWindow,
            ..Default::default()
        },
        cx,
    );
    cx.spawn(async move |cx| {
        let result = open_task.await?;
        result.window.update(cx, |_, window, cx| {
            result.workspace.update(cx, |workspace, cx| {
                open(workspace, Location::PrimaryEditor, None, window, cx);
            });
        })?;
        anyhow::Ok(())
    })
    .detach_and_log_err(cx);
}
