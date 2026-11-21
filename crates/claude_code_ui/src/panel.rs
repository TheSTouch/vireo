use std::path::PathBuf;

use anyhow::Result;
use gpui::{
    Action, App, AsyncWindowContext, Context, Entity, EventEmitter, FocusHandle, Focusable, Pixels,
    SharedString, Subscription, Task, WeakEntity, Window, prelude::*, px,
};
use project::Project;
use settings::{ClaudeCodeOpensIn, ClaudeCodePreferredLocation, Settings as _};
use ui::prelude::*;
use util::ResultExt as _;
use workspace::dock::{DockPosition, Panel, PanelEvent};
use workspace::item::{Item, ItemEvent};
use workspace::{OpenOptions, Workspace};

use crate::session::ClaudeSession;
use crate::settings::ClaudeCodeSettings;
use crate::view::{ClaudeCodeView, ViewEvent};
use crate::{NewConversation, ToggleFocus};

const PANEL_KEY: &str = "ClaudeCodePanel";

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
        {
            let session = view.read(cx).session.clone();
            session.update(cx, |session, cx| {
                if session.may_continue(cx) {
                    session.continue_interrupted_turn(cx);
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
        if conversation_is_empty(&self.view, cx) {
            window.focus(&self.view.read(cx).input_focus_handle(cx), cx);
            return;
        }
        if ClaudeCodeSettings::get_global(cx).new_conversation_opens_in
            == ClaudeCodeOpensIn::NewTab
        {
            open_in_new_tab_later(self.workspace.clone(), None, window, cx);
            return;
        }
        // Not the workspace's project: this can run inside an update of the
        // workspace, which cannot be read then.
        let Some(project) = self.view.read(cx).session.read(cx).project() else {
            return;
        };
        self.replace_conversation(&workspace, project, None, window, cx);
    }

    /// Resumes the newest conversation of the project from `transcripts_dir`,
    /// unless the panel already shows one, such as the session it had before
    /// Vireo restarted.
    pub fn resume_newest_conversation(
        &mut self,
        transcripts_dir: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.shows_new_conversation(cx) {
            return;
        }
        let store = crate::sessions::read_store(cx);
        let newest = cx.background_spawn(async move {
            crate::sessions::newest_session(&transcripts_dir, &store)
        });
        cx.spawn_in(window, async move |this, cx| {
            let Some(session_id) = newest.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                // The person may have started typing a new one meanwhile.
                if !this.shows_new_conversation(cx) {
                    return;
                }
                let Some(workspace) = this.workspace.upgrade() else {
                    return;
                };
                let Some(project) = this.view.read(cx).session.read(cx).project() else {
                    return;
                };
                this.replace_conversation(&workspace, project, Some(session_id), window, cx);
            })
            .log_err();
        })
        .detach();
    }

    fn shows_new_conversation(&self, cx: &App) -> bool {
        let session = self.view.read(cx).session.read(cx);
        session.session_id().is_none() && session.conversation.entries.is_empty()
    }

    /// Shows a new conversation, or the past session `resume`, in place of the
    /// panel's conversation.
    pub fn replace_conversation(
        &mut self,
        workspace: &Entity<Workspace>,
        project: Entity<Project>,
        resume: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let old_session = self.view.read(cx).session.clone();
        crate::sessions::remember_closed(&old_session, cx);
        let view = new_view(workspace, project, resume, window, cx);
        self.set_view(view, cx);
        window.focus(&self.view.read(cx).input_focus_handle(cx), cx);
    }
}

fn conversation_is_empty(view: &Entity<ClaudeCodeView>, cx: &App) -> bool {
    view.read(cx)
        .session
        .read(cx)
        .conversation
        .entries
        .is_empty()
}

/// Opens a conversation in a new tab once the current update has ended: the
/// panel or tab asking for it may be inside an update of the workspace, or be
/// one of the items the pane updates while it adds the new tab.
fn open_in_new_tab_later(
    workspace: WeakEntity<Workspace>,
    resume: Option<String>,
    window: &mut Window,
    cx: &mut App,
) {
    window.defer(cx, move |window, cx| {
        workspace
            .update(cx, |workspace, cx| {
                open(workspace, Location::NewTab, resume, window, cx);
            })
            .log_err();
    });
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
        if conversation_is_empty(&self.view, cx) {
            window.focus(&self.view.read(cx).input_focus_handle(cx), cx);
            return;
        }
        if ClaudeCodeSettings::get_global(cx).new_conversation_opens_in
            == ClaudeCodeOpensIn::NewTab
        {
            open_in_new_tab_later(self.workspace.clone(), None, window, cx);
            return;
        }
        // Not the workspace's project: this can run inside an update of the
        // workspace, which cannot be read then.
        let Some(project) = self.view.read(cx).session.read(cx).project() else {
            return;
        };
        self.replace_conversation(&workspace, project, None, window, cx);
    }

    /// Shows a new conversation, or the past session `resume`, in place of the
    /// tab's conversation.
    pub fn replace_conversation(
        &mut self,
        workspace: &Entity<Workspace>,
        project: Entity<Project>,
        resume: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let old_session = self.view.read(cx).session.clone();
        crate::sessions::remember_closed(&old_session, cx);
        let view = new_view(workspace, project, resume, window, cx);
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

/// Brings forward the tab or panel that already shows `session_id`, so a
/// past conversation is never open twice.
pub fn focus_open_session(
    workspace: &mut Workspace,
    session_id: &str,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> bool {
    let shows_session = |view: &Entity<ClaudeCodeView>, cx: &App| {
        view.read(cx).session.read(cx).session_id() == Some(session_id)
    };
    let tab = workspace
        .items_of_type::<ClaudeCodeTab>(cx)
        .find(|tab| shows_session(&tab.read(cx).view, cx));
    if let Some(tab) = tab {
        let view = tab.read(cx).view.clone();
        workspace.activate_item(&tab, true, true, window, cx);
        window.focus(&view.read(cx).input_focus_handle(cx), cx);
        return true;
    }
    let panel_shows_session = workspace
        .panel::<ClaudeCodePanel>(cx)
        .is_some_and(|panel| shows_session(&panel.read(cx).view, cx));
    if panel_shows_session {
        open(workspace, Location::Panel, None, window, cx);
        return true;
    }
    false
}

/// Where a past conversation picked from the list opens.
#[derive(Clone)]
pub enum PastConversationTarget {
    NewTab,
    Panel,
    Tab(WeakEntity<ClaudeCodeTab>),
}

/// Opens a past conversation picked from the list, or brings forward the view
/// that already shows it.
pub fn open_past_conversation(
    workspace: &mut Workspace,
    session_id: String,
    target: PastConversationTarget,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    if focus_open_session(workspace, &session_id, window, cx) {
        return;
    }
    match target {
        PastConversationTarget::NewTab => {
            open(workspace, Location::NewTab, Some(session_id), window, cx);
        }
        PastConversationTarget::Panel => {
            open(workspace, Location::Panel, Some(session_id), window, cx);
        }
        PastConversationTarget::Tab(tab) => {
            let Some(tab) = tab.upgrade() else {
                open(workspace, Location::NewTab, Some(session_id), window, cx);
                return;
            };
            let workspace_entity = cx.entity();
            let project = workspace.project().clone();
            workspace.activate_item(&tab, true, true, window, cx);
            tab.update(cx, |tab, cx| {
                tab.replace_conversation(&workspace_entity, project, Some(session_id), window, cx)
            });
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use gpui::{TestAppContext, VisualTestContext};
    use settings::SettingsStore;
    use util::path;

    fn init(cx: &mut TestAppContext) -> std::sync::Arc<workspace::AppState> {
        cx.update(|cx| {
            let state = workspace::AppState::test(cx);
            theme_settings::init(theme::LoadThemes::JustBase, cx);
            editor::init(cx);
            ClaudeCodeSettings::register(cx);
            crate::sessions::update_store(cx, |store| *store = Default::default());
            state
        })
    }

    async fn workspace_with_panel(
        cx: &mut TestAppContext,
    ) -> (
        Entity<Workspace>,
        Entity<ClaudeCodePanel>,
        &mut VisualTestContext,
    ) {
        let app_state = init(cx);
        app_state
            .fs
            .as_fake()
            .insert_tree(path!("/root"), serde_json::json!({ "a.rs": "" }))
            .await;
        let project = Project::test(app_state.fs.clone(), [path!("/root").as_ref()], cx).await;
        let (multi_workspace, cx) = cx
            .add_window_view(|window, cx| workspace::MultiWorkspace::test_new(project, window, cx));
        let workspace =
            multi_workspace.read_with(cx, |multi_workspace, _| multi_workspace.workspace().clone());
        let panel = workspace.update_in(cx, |workspace, window, cx| {
            let project = workspace.project().clone();
            let workspace_entity = cx.entity();
            let panel = cx.new(|cx| ClaudeCodePanel::new(workspace_entity, project, window, cx));
            workspace.add_panel(panel.clone(), window, cx);
            panel
        });
        cx.run_until_parked();
        (workspace, panel, cx)
    }

    fn set_opens_in(
        new_conversation: ClaudeCodeOpensIn,
        past_conversation: ClaudeCodeOpensIn,
        cx: &mut VisualTestContext,
    ) {
        cx.update(|_, cx| {
            cx.update_global::<SettingsStore, ()>(|store, cx| {
                store.update_user_settings(cx, |settings| {
                    let claude = settings.claude_code.get_or_insert_default();
                    claude.new_conversation_opens_in = Some(new_conversation);
                    claude.past_conversation_opens_in = Some(past_conversation);
                });
            });
        });
    }

    fn start_conversation(view: &Entity<ClaudeCodeView>, session_id: &str, cx: &mut App) {
        let session = view.read(cx).session.clone();
        session.update(cx, |session, _| {
            session.conversation.session_id = Some(session_id.to_string());
            session
                .conversation
                .push_user_prompt("hello".into(), 0, Utc::now());
        });
    }

    fn tabs(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Vec<Entity<ClaudeCodeTab>> {
        workspace.read_with(cx, |workspace, cx| {
            workspace.items_of_type::<ClaudeCodeTab>(cx).collect()
        })
    }

    fn session_id(view: &Entity<ClaudeCodeView>, cx: &mut VisualTestContext) -> Option<String> {
        view.read_with(cx, |view, cx| {
            view.session.read(cx).session_id().map(str::to_string)
        })
    }

    #[gpui::test]
    async fn new_conversation_from_the_panel_opens_a_tab(cx: &mut TestAppContext) {
        let (workspace, panel, cx) = workspace_with_panel(cx).await;
        let panel_view = panel.read_with(cx, |panel, _| panel.view.clone());

        // An empty conversation is only focused.
        panel.update_in(cx, |panel, window, cx| panel.new_conversation(window, cx));
        cx.run_until_parked();
        assert!(tabs(&workspace, cx).is_empty());

        cx.update(|_, cx| start_conversation(&panel_view, "s1", cx));
        panel.update_in(cx, |panel, window, cx| panel.new_conversation(window, cx));
        cx.run_until_parked();
        assert_eq!(tabs(&workspace, cx).len(), 1);
        panel.read_with(cx, |panel, _| assert_eq!(panel.view, panel_view));
        assert_eq!(session_id(&panel_view, cx).as_deref(), Some("s1"));

        set_opens_in(ClaudeCodeOpensIn::Current, ClaudeCodeOpensIn::Current, cx);
        panel.update_in(cx, |panel, window, cx| panel.new_conversation(window, cx));
        cx.run_until_parked();
        assert_eq!(tabs(&workspace, cx).len(), 1);
        let replaced = panel.read_with(cx, |panel, _| panel.view.clone());
        assert_ne!(replaced, panel_view);
        assert_eq!(session_id(&replaced, cx), None);
    }

    #[gpui::test]
    async fn new_conversation_from_a_tab_opens_another_tab(cx: &mut TestAppContext) {
        let (workspace, _panel, cx) = workspace_with_panel(cx).await;
        let view = workspace
            .update_in(cx, |workspace, window, cx| {
                open(workspace, Location::NewTab, None, window, cx)
            })
            .unwrap();
        cx.run_until_parked();
        let tab = tabs(&workspace, cx)[0].clone();

        tab.update_in(cx, |tab, window, cx| tab.new_conversation(window, cx));
        cx.run_until_parked();
        assert_eq!(tabs(&workspace, cx).len(), 1, "an empty tab is only focused");

        cx.update(|_, cx| start_conversation(&view, "s1", cx));
        tab.update_in(cx, |tab, window, cx| tab.new_conversation(window, cx));
        cx.run_until_parked();
        assert_eq!(tabs(&workspace, cx).len(), 2);
        tab.read_with(cx, |tab, _| assert_eq!(tab.view, view));

        set_opens_in(ClaudeCodeOpensIn::Current, ClaudeCodeOpensIn::Current, cx);
        tab.update_in(cx, |tab, window, cx| tab.new_conversation(window, cx));
        cx.run_until_parked();
        assert_eq!(tabs(&workspace, cx).len(), 2);
        let replaced = tab.read_with(cx, |tab, _| tab.view.clone());
        assert_ne!(replaced, view);
        assert_eq!(session_id(&replaced, cx), None);
    }

    #[gpui::test]
    async fn past_conversations_open_in_a_tab_once(cx: &mut TestAppContext) {
        let (workspace, panel, cx) = workspace_with_panel(cx).await;
        let panel_view = panel.read_with(cx, |panel, _| panel.view.clone());
        cx.update(|_, cx| start_conversation(&panel_view, "s1", cx));

        let pick = |session_id: &str, cx: &mut VisualTestContext| {
            let session_id = session_id.to_string();
            workspace.update_in(cx, |workspace, window, cx| {
                let target = crate::past_conversation_target(workspace, window, cx);
                open_past_conversation(workspace, session_id, target, window, cx);
            });
            cx.run_until_parked();
        };

        pick("s2", cx);
        let opened = tabs(&workspace, cx);
        assert_eq!(opened.len(), 1);
        let tab_view = opened[0].read_with(cx, |tab, _| tab.view.clone());
        assert_eq!(session_id(&tab_view, cx).as_deref(), Some("s2"));
        panel.read_with(cx, |panel, _| assert_eq!(panel.view, panel_view));

        // Already open: brought forward, not opened twice.
        pick("s2", cx);
        pick("s1", cx);
        assert_eq!(tabs(&workspace, cx).len(), 1);
        panel.read_with(cx, |panel, _| assert_eq!(panel.view, panel_view));

        // "current" from a tab replaces that tab's conversation.
        set_opens_in(ClaudeCodeOpensIn::Current, ClaudeCodeOpensIn::Current, cx);
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.activate_item(&opened[0], true, true, window, cx);
        });
        pick("s3", cx);
        let opened = tabs(&workspace, cx);
        assert_eq!(opened.len(), 1);
        let tab_view = opened[0].read_with(cx, |tab, _| tab.view.clone());
        assert_eq!(session_id(&tab_view, cx).as_deref(), Some("s3"));

        // "current" from the panel replaces the panel's conversation.
        workspace.update_in(cx, |workspace, window, cx| {
            open(workspace, Location::Panel, None, window, cx);
        });
        pick("s4", cx);
        assert_eq!(tabs(&workspace, cx).len(), 1);
        let panel_view = panel.read_with(cx, |panel, _| panel.view.clone());
        assert_eq!(session_id(&panel_view, cx).as_deref(), Some("s4"));
    }

    fn write_transcript(dir: &std::path::Path, session_id: &str, age_seconds: u64) {
        let path = dir.join(format!("{session_id}.jsonl"));
        std::fs::write(
            &path,
            format!(
                r#"{{"type":"user","sessionId":"{session_id}","message":{{"role":"user","content":"Prompt of {session_id}"}}}}"#
            ),
        )
        .unwrap();
        let modified = std::time::SystemTime::now() - std::time::Duration::from_secs(age_seconds);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
    }

    fn right_dock_shows_panel(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> bool {
        workspace.read_with(cx, |workspace, cx| {
            let dock = workspace.right_dock().read(cx);
            dock.is_open()
                && dock
                    .visible_panel()
                    .is_some_and(|panel| panel.panel_key() == PANEL_KEY)
        })
    }

    fn open_project(
        workspace: &Entity<Workspace>,
        transcripts: &tempfile::TempDir,
        cx: &mut VisualTestContext,
    ) {
        let dir = transcripts.path().to_path_buf();
        workspace.update_in(cx, |workspace, window, cx| {
            crate::open_on_project_open_from(workspace, dir, window, cx)
        });
        cx.run_until_parked();
    }

    fn input_is_focused(view: &Entity<ClaudeCodeView>, cx: &mut VisualTestContext) -> bool {
        cx.update(|window, cx| view.read(cx).input_focus_handle(cx).is_focused(window))
    }

    #[gpui::test]
    async fn a_project_opens_with_its_newest_conversation(cx: &mut TestAppContext) {
        let (workspace, panel, cx) = workspace_with_panel(cx).await;
        let transcripts = tempfile::tempdir().unwrap();
        write_transcript(transcripts.path(), "old", 3600);
        write_transcript(transcripts.path(), "newest", 60);
        write_transcript(transcripts.path(), "archived", 0);
        cx.update(|_, cx| {
            crate::sessions::update_store(cx, |store| store.meta_mut("archived").archived = true)
        });
        assert!(!right_dock_shows_panel(&workspace, cx));

        open_project(&workspace, &transcripts, cx);
        assert!(right_dock_shows_panel(&workspace, cx));
        let view = panel.read_with(cx, |panel, _| panel.view.clone());
        assert_eq!(session_id(&view, cx).as_deref(), Some("newest"));
        assert!(input_is_focused(&view, cx));
    }

    #[gpui::test]
    async fn a_project_without_conversations_opens_a_new_one(cx: &mut TestAppContext) {
        let (workspace, panel, cx) = workspace_with_panel(cx).await;
        let transcripts = tempfile::tempdir().unwrap();
        let empty_view = panel.read_with(cx, |panel, _| panel.view.clone());

        open_project(&workspace, &transcripts, cx);
        assert!(right_dock_shows_panel(&workspace, cx));
        let view = panel.read_with(cx, |panel, _| panel.view.clone());
        assert_eq!(view, empty_view);
        assert_eq!(session_id(&view, cx), None);
        assert!(input_is_focused(&view, cx));
    }

    #[gpui::test]
    async fn the_restored_session_and_the_setting_win(cx: &mut TestAppContext) {
        let (workspace, panel, cx) = workspace_with_panel(cx).await;
        let transcripts = tempfile::tempdir().unwrap();
        write_transcript(transcripts.path(), "newest", 0);

        cx.update(|_, cx| {
            cx.update_global::<SettingsStore, ()>(|store, cx| {
                store.update_user_settings(cx, |settings| {
                    settings.claude_code.get_or_insert_default().open_on_project_open = Some(false);
                });
            });
        });
        open_project(&workspace, &transcripts, cx);
        assert!(!right_dock_shows_panel(&workspace, cx));

        cx.update(|_, cx| {
            cx.update_global::<SettingsStore, ()>(|store, cx| {
                store.update_user_settings(cx, |settings| {
                    settings.claude_code.get_or_insert_default().open_on_project_open = Some(true);
                });
            });
        });
        // The session the panel had before a restart stays.
        let restored = panel.read_with(cx, |panel, _| panel.view.clone());
        cx.update(|_, cx| start_conversation(&restored, "restored", cx));
        open_project(&workspace, &transcripts, cx);
        assert!(right_dock_shows_panel(&workspace, cx));
        panel.read_with(cx, |panel, _| assert_eq!(panel.view, restored));
        assert_eq!(session_id(&restored, cx).as_deref(), Some("restored"));
    }
}
