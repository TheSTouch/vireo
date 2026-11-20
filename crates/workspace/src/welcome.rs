use crate::{
    NewFile, Open, OpenMode, PathList, RecentWorkspace, SerializedWorkspaceLocation,
    ToggleWorkspaceSidebar, Workspace, WorkspaceSettings,
    item::{Item, ItemEvent},
    persistence::WorkspaceDb,
};
use agent_settings::AgentSettings;
use git::Clone as GitClone;
use gpui::{
    Action, App, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    ParentElement, Render, Styled, Task, TaskExt, Window, actions,
};
use gpui::{WeakEntity, linear_color_stop, linear_gradient};
use menu::{SelectNext, SelectPrevious};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use settings::{DefaultOpenBehavior, Settings};
use ui::{ButtonLike, KeyBinding, prelude::*};
use util::{ResultExt, paths::PathExt as _};
use zed_actions::{
    Extensions, OpenOnboarding, OpenRecent, OpenRemote, OpenSettings, assistant::ToggleFocus,
};

/// The welcome page's title. VS Code shows its own name here; Vireo shows
/// its own, with no logo.
const WELCOME_TITLE: &str = "Vireo";
const WELCOME_SUBTITLE: &str = "A code editor built for speed";
const RECENT_LIMIT: usize = 5;

#[derive(PartialEq, Clone, Debug, Deserialize, Serialize, JsonSchema, Action)]
#[action(namespace = welcome)]
#[serde(transparent)]
pub struct OpenRecentProject {
    pub index: usize,
}

actions!(
    zed,
    [
        /// Show the welcome screen
        ShowWelcome
    ]
);

#[derive(IntoElement)]
struct SectionHeader {
    title: SharedString,
}

impl SectionHeader {
    fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
        }
    }
}

impl RenderOnce for SectionHeader {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        // VS Code titles its welcome sections plainly: "Start", "Recent".
        h_flex().px_1().mb_1().child(
            Label::new(self.title)
                .size(LabelSize::Custom(rems_from_px(19_f32)))
                .weight(gpui::FontWeight::MEDIUM),
        )
    }
}

#[derive(IntoElement)]
struct SectionButton {
    label: SharedString,
    icon: IconName,
    action: Box<dyn Action>,
    tab_index: usize,
    focus_handle: FocusHandle,
}

impl SectionButton {
    fn new(
        label: impl Into<SharedString>,
        icon: IconName,
        action: &dyn Action,
        tab_index: usize,
        focus_handle: FocusHandle,
    ) -> Self {
        Self {
            label: label.into(),
            icon,
            action: action.boxed_clone(),
            tab_index,
            focus_handle,
        }
    }
}

impl RenderOnce for SectionButton {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let id = format!("onb-button-{}-{}", self.label, self.tab_index);

        // VS Code's welcome entries are links: an accent icon and label.
        ButtonLike::new(id)
            .tab_index(self.tab_index as isize)
            .size(ButtonSize::Medium)
            .child(
                h_flex()
                    .min_w_0()
                    .gap_2()
                    .child(
                        Icon::new(self.icon)
                            .color(Color::Accent)
                            .size(IconSize::Small),
                    )
                    .child(Label::new(self.label).color(Color::Accent).truncate()),
            )
            .on_click(move |_, window, cx| {
                self.focus_handle.dispatch_action(&*self.action, window, cx)
            })
    }
}

enum SectionVisibility {
    Always,
}

impl SectionVisibility {
    fn is_visible(&self) -> bool {
        match self {
            SectionVisibility::Always => true,
        }
    }
}

struct SectionEntry {
    icon: IconName,
    title: &'static str,
    action: &'static dyn Action,
    visibility_guard: SectionVisibility,
}

impl SectionEntry {
    fn render(&self, button_index: usize, focus: &FocusHandle) -> Option<impl IntoElement> {
        self.visibility_guard.is_visible().then(|| {
            SectionButton::new(
                self.title,
                self.icon,
                self.action,
                button_index,
                focus.clone(),
            )
        })
    }
}

/// VS Code's "Start" section.
const START: Section<4> = Section {
    title: "Start",
    entries: [
        SectionEntry {
            icon: IconName::Plus,
            title: "New File...",
            action: &NewFile,
            visibility_guard: SectionVisibility::Always,
        },
        SectionEntry {
            icon: IconName::FolderOpen,
            title: "Open Folder...",
            action: &Open::DEFAULT,
            visibility_guard: SectionVisibility::Always,
        },
        SectionEntry {
            icon: IconName::GitBranch,
            title: "Clone Git Repository...",
            action: &GitClone,
            visibility_guard: SectionVisibility::Always,
        },
        SectionEntry {
            icon: IconName::Remote,
            title: "Connect to...",
            action: &OpenRemote {
                from_existing_connection: false,
                create_new_window: None,
            },
            visibility_guard: SectionVisibility::Always,
        },
    ],
};

/// A walkthrough card in the right column. VS Code's walkthroughs are
/// step-by-step guides; until Vireo has its own, each card opens the page
/// that covers its topic.
struct Walkthrough {
    icon: IconName,
    title: &'static str,
    description: &'static str,
    action: &'static dyn Action,
}

const WALKTHROUGHS: [Walkthrough; 3] = [
    Walkthrough {
        icon: IconName::Sparkle,
        title: "Get Started with Vireo",
        description: "Pick a theme, a keymap and the basics.",
        action: &OpenOnboarding,
    },
    Walkthrough {
        icon: IconName::Settings,
        title: "Customize Your Editor",
        description: "Settings and keyboard shortcuts.",
        action: &OpenSettings,
    },
    Walkthrough {
        icon: IconName::Blocks,
        title: "Add Features with Add-ons",
        description: "Languages, themes and tools.",
        action: &Extensions {
            category_filter: None,
            id: None,
        },
    },
];

struct Section<const COLS: usize> {
    title: &'static str,
    entries: [SectionEntry; COLS],
}

impl<const COLS: usize> Section<COLS> {
    fn render(self, index_offset: usize, focus: &FocusHandle) -> impl IntoElement {
        v_flex()
            .min_w_full()
            .child(SectionHeader::new(self.title))
            .children(
                self.entries
                    .iter()
                    .enumerate()
                    .filter_map(|(index, entry)| entry.render(index_offset + index, focus)),
            )
    }
}

pub struct WelcomePage {
    workspace: WeakEntity<Workspace>,
    focus_handle: FocusHandle,
    fallback_to_recent_projects: bool,
    recent_workspaces: Option<Vec<RecentWorkspace>>,
}

impl WelcomePage {
    pub fn new(
        workspace: WeakEntity<Workspace>,
        fallback_to_recent_projects: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        cx.on_focus(&focus_handle, window, |_, _, cx| cx.notify())
            .detach();

        // VS Code lists recent folders on every welcome page. The page can be
        // created while the workspace is being updated, so the workspace is
        // read once that update is over, in the task.
        {
            let db = WorkspaceDb::global(cx);
            cx.spawn_in(window, async move |this: WeakEntity<Self>, cx| {
                let Some(fs) = this
                    .update(cx, |this, cx| {
                        this.workspace
                            .upgrade()
                            .map(|workspace| workspace.read(cx).app_state().fs.clone())
                    })
                    .ok()
                    .flatten()
                else {
                    return;
                };
                let workspaces = db
                    .recent_project_workspaces(fs.as_ref())
                    .await
                    .log_err()
                    .unwrap_or_default();

                this.update(cx, |this, cx| {
                    this.recent_workspaces = Some(workspaces);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }

        WelcomePage {
            workspace,
            focus_handle,
            fallback_to_recent_projects,
            recent_workspaces: None,
        }
    }

    fn select_next(&mut self, _: &SelectNext, window: &mut Window, cx: &mut Context<Self>) {
        window.focus_next(cx);
        cx.notify();
    }

    fn select_previous(&mut self, _: &SelectPrevious, window: &mut Window, cx: &mut Context<Self>) {
        window.focus_prev(cx);
        cx.notify();
    }

    fn open_recent_project(
        &mut self,
        action: &OpenRecentProject,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(recent_workspaces) = &self.recent_workspaces {
            if let Some(workspace) = recent_workspaces.get(action.index) {
                let is_local = matches!(workspace.location, SerializedWorkspaceLocation::Local);

                if is_local {
                    let paths = workspace.paths.paths().to_vec();
                    let open_mode = match WorkspaceSettings::get_global(cx).default_open_behavior {
                        DefaultOpenBehavior::ExistingWindow => OpenMode::Activate,
                        DefaultOpenBehavior::NewWindow => OpenMode::NewWindow,
                    };
                    self.workspace
                        .update(cx, |workspace, cx| {
                            workspace
                                .open_workspace_for_paths(open_mode, paths, window, cx)
                                .detach_and_log_err(cx);
                        })
                        .log_err();
                } else {
                    window.dispatch_action(OpenRecent::default().boxed_clone(), cx);
                }
            }
        }
    }

    fn render_agent_card(&self, tab_index: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let focus = self.focus_handle.clone();
        let color = cx.theme().colors();

        let description = "Run multiple threads at once, mix and match any ACP-compatible agent, and keep work conflict-free with worktrees.";

        v_flex()
            .w_full()
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(color.border_variant)
            .bg(linear_gradient(
                360.,
                linear_color_stop(color.panel_background, 1.0),
                linear_color_stop(color.editor_background, 0.45),
            ))
            .child(
                h_flex()
                    .gap_1p5()
                    .child(
                        Icon::new(IconName::ZedAssistant)
                            .color(Color::Muted)
                            .size(IconSize::Small),
                    )
                    .child(Label::new("Collaborate with Agents")),
            )
            .child(
                Label::new(description)
                    .size(LabelSize::Small)
                    .color(Color::Muted)
                    .mb_2(),
            )
            .child(
                Button::new("open-agent", "Open Agent Panel")
                    .full_width()
                    .tab_index(tab_index as isize)
                    .style(ButtonStyle::Outlined)
                    .key_binding(
                        KeyBinding::for_action_in(&ToggleFocus, &self.focus_handle, cx)
                            .size(rems_from_px(12_f32)),
                    )
                    .on_click(move |_, window, cx| {
                        focus.dispatch_action(&ToggleWorkspaceSidebar, window, cx);
                        focus.dispatch_action(&ToggleFocus, window, cx);
                    }),
            )
    }

    /// VS Code's "Recent" section: each folder as a link followed by its
    /// path, then "More..." for the full list.
    fn render_recent_section(&self, first_tab_index: usize) -> (impl IntoElement, usize) {
        let recents = self
            .recent_workspaces
            .as_ref()
            .into_iter()
            .flatten()
            .take(RECENT_LIMIT)
            .collect::<Vec<_>>();
        let count = recents.len();
        let focus_handle = self.focus_handle.clone();
        let section = v_flex()
            .w_full()
            .child(SectionHeader::new("Recent"))
            .when(recents.is_empty(), |this| {
                this.child(
                    div().px_1().child(
                        Label::new("You have no recent folders, open a folder to start.")
                            .color(Color::Muted),
                    ),
                )
            })
            .children(recents.into_iter().enumerate().map(|(index, workspace)| {
                self.render_recent_project(
                    index,
                    first_tab_index + index,
                    &workspace.location,
                    &workspace.identity_paths,
                )
            }))
            .when(count > 0, |this| {
                this.child(
                    ButtonLike::new("welcome-more-recent")
                        .tab_index((first_tab_index + count) as isize)
                        .child(Label::new("More...").color(Color::Accent))
                        .on_click(move |_, window, cx| {
                            focus_handle.dispatch_action(&OpenRecent::default(), window, cx)
                        }),
                )
            });
        let used_tab_indices = if count > 0 { count + 1 } else { 0 };
        (section, used_tab_indices)
    }

    fn render_recent_project(
        &self,
        project_index: usize,
        tab_index: usize,
        location: &SerializedWorkspaceLocation,
        paths: &PathList,
    ) -> impl IntoElement {
        let name = project_name(paths);
        let parent = recent_project_parent(paths);
        let icon = match location {
            SerializedWorkspaceLocation::Local => IconName::Folder,
            SerializedWorkspaceLocation::Remote(_) => IconName::Server,
        };
        let focus_handle = self.focus_handle.clone();
        let action = OpenRecentProject {
            index: project_index,
        };
        ButtonLike::new(("welcome-recent", project_index))
            .tab_index(tab_index as isize)
            .child(
                h_flex()
                    .min_w_0()
                    .gap_3()
                    .when(icon == IconName::Server, |this| {
                        this.child(Icon::new(icon).color(Color::Accent).size(IconSize::Small))
                    })
                    .child(Label::new(name).color(Color::Accent))
                    .children(
                        parent.map(|parent| Label::new(parent).color(Color::Muted).truncate()),
                    ),
            )
            .on_click(move |_, window, cx| focus_handle.dispatch_action(&action, window, cx))
    }

    fn render_walkthroughs(&self, first_tab_index: usize) -> impl IntoElement {
        let colors_border = |cx: &App| cx.theme().colors().border_variant;
        let focus_handle = self.focus_handle.clone();
        v_flex()
            .w_full()
            .gap_2()
            .child(SectionHeader::new("Walkthroughs"))
            .children(
                WALKTHROUGHS
                    .iter()
                    .enumerate()
                    .map(move |(index, walkthrough)| {
                        let focus_handle = focus_handle.clone();
                        let action = walkthrough.action.boxed_clone();
                        WalkthroughCard {
                            index,
                            tab_index: first_tab_index + index,
                            icon: walkthrough.icon,
                            title: walkthrough.title,
                            description: walkthrough.description,
                            on_click: Box::new(move |window, cx| {
                                focus_handle.dispatch_action(action.as_ref(), window, cx)
                            }),
                            border: colors_border,
                        }
                    }),
            )
    }
}

#[derive(IntoElement)]
struct WalkthroughCard {
    index: usize,
    tab_index: usize,
    icon: IconName,
    title: &'static str,
    description: &'static str,
    on_click: Box<dyn Fn(&mut Window, &mut App)>,
    border: fn(&App) -> gpui::Hsla,
}

impl RenderOnce for WalkthroughCard {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let on_click = self.on_click;
        // VS Code's walkthrough tiles: a filled, rounded card with the title
        // over a one-line description.
        let text = cx.theme().colors().text;
        h_flex()
            .id(("welcome-walkthrough", self.index))
            .tab_index(self.tab_index as isize)
            .w_full()
            .px_3()
            .py_2()
            .gap_3()
            .items_start()
            .rounded_md()
            .border_1()
            .border_color((self.border)(cx))
            .bg(text.opacity(0.07))
            .hover(move |style| style.bg(text.opacity(0.11)))
            .cursor_pointer()
            .child(Icon::new(self.icon).color(Color::Accent))
            .child(
                v_flex()
                    .min_w_0()
                    .child(Label::new(self.title).weight(gpui::FontWeight::SEMIBOLD))
                    .child(Label::new(self.description).color(Color::Muted)),
            )
            .on_click(move |_, window, cx| on_click(window, cx))
    }
}

/// The folder that holds a recent project, shortened with `~`, as VS Code
/// shows next to the project's name.
fn recent_project_parent(paths: &PathList) -> Option<String> {
    let first = paths.paths().first()?;
    let parent = first.parent()?;
    Some(parent.compact().to_string_lossy().into_owned())
}

impl Render for WelcomePage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let start_entries = START.entries.len();
        let (recent_section, recent_tab_indices) = self.render_recent_section(start_entries);
        let walkthrough_tab_index = start_entries + recent_tab_indices;
        let agent_tab_index = walkthrough_tab_index + WALKTHROUGHS.len();
        let ai_enabled = AgentSettings::get_global(cx).enabled(cx);

        h_flex()
            .key_context("Welcome")
            .track_focus(&self.focus_handle(cx))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::open_recent_project))
            .size_full()
            .bg(cx.theme().colors().editor_background)
            .justify_center()
            .child(
                v_flex()
                    .id("welcome-content")
                    .px_8()
                    .py_12()
                    .max_w(px(1000.))
                    .size_full()
                    .gap_10()
                    .overflow_y_scroll()
                    // VS Code's header: the product's name, large and light,
                    // over a one-line subtitle.
                    .child(
                        v_flex()
                            .child(
                                Label::new(WELCOME_TITLE)
                                    .size(LabelSize::Custom(rems_from_px(36_f32)))
                                    .weight(gpui::FontWeight::LIGHT),
                            )
                            .child(
                                Label::new(WELCOME_SUBTITLE)
                                    .size(LabelSize::Large)
                                    .color(Color::Muted),
                            ),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .items_start()
                            .gap_16()
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_8()
                                    .child(START.render(0, &self.focus_handle))
                                    .child(recent_section),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_4()
                                    .child(self.render_walkthroughs(walkthrough_tab_index))
                                    .when(
                                        ai_enabled && !self.fallback_to_recent_projects,
                                        |this| {
                                            this.child(self.render_agent_card(agent_tab_index, cx))
                                        },
                                    ),
                            ),
                    ),
            )
    }
}

impl EventEmitter<ItemEvent> for WelcomePage {}

impl Focusable for WelcomePage {
    fn focus_handle(&self, _: &App) -> gpui::FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for WelcomePage {
    type Event = ItemEvent;

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        "Welcome".into()
    }

    fn telemetry_event_text(&self) -> Option<&'static str> {
        Some("New Welcome Page Opened")
    }

    fn show_toolbar(&self) -> bool {
        false
    }

    fn to_item_events(event: &Self::Event, f: &mut dyn FnMut(crate::item::ItemEvent)) {
        f(*event)
    }
}

impl crate::SerializableItem for WelcomePage {
    fn serialized_item_kind() -> &'static str {
        "WelcomePage"
    }

    fn cleanup(
        workspace_id: crate::WorkspaceId,
        alive_items: Vec<crate::ItemId>,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<gpui::Result<()>> {
        crate::delete_unloaded_items(
            alive_items,
            workspace_id,
            "welcome_pages",
            &persistence::WelcomePagesDb::global(cx),
            cx,
        )
    }

    fn deserialize(
        _project: Entity<project::Project>,
        workspace: gpui::WeakEntity<Workspace>,
        workspace_id: crate::WorkspaceId,
        item_id: crate::ItemId,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<gpui::Result<Entity<Self>>> {
        if persistence::WelcomePagesDb::global(cx)
            .get_welcome_page(item_id, workspace_id)
            .ok()
            .is_some_and(|is_open| is_open)
        {
            Task::ready(Ok(
                cx.new(|cx| WelcomePage::new(workspace, false, window, cx))
            ))
        } else {
            Task::ready(Err(anyhow::anyhow!("No welcome page to deserialize")))
        }
    }

    fn serialize(
        &mut self,
        workspace: &mut Workspace,
        item_id: crate::ItemId,
        _closing: bool,
        cx: &mut Context<Self>,
    ) -> Option<Task<gpui::Result<()>>> {
        let workspace_id = workspace.database_id()?;
        let db = persistence::WelcomePagesDb::global(cx);
        Some(cx.background_spawn(
            async move { db.save_welcome_page(item_id, workspace_id, true).await },
        ))
    }

    fn should_serialize(&self, event: &Self::Event) -> bool {
        event == &ItemEvent::UpdateTab
    }
}

mod persistence {
    use crate::WorkspaceDb;
    use db::{
        query,
        sqlez::{domain::Domain, thread_safe_connection::ThreadSafeConnection},
        sqlez_macros::sql,
    };

    pub struct WelcomePagesDb(ThreadSafeConnection);

    impl Domain for WelcomePagesDb {
        const NAME: &str = stringify!(WelcomePagesDb);

        const MIGRATIONS: &[&str] = (&[sql!(
                    CREATE TABLE welcome_pages (
                        workspace_id INTEGER,
                        item_id INTEGER UNIQUE,
                        is_open INTEGER DEFAULT FALSE,

                        PRIMARY KEY(workspace_id, item_id),
                        FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id)
                        ON DELETE CASCADE
                    ) STRICT;
        )]);
    }

    db::static_connection!(WelcomePagesDb, [WorkspaceDb]);

    impl WelcomePagesDb {
        query! {
            pub async fn save_welcome_page(
                item_id: crate::ItemId,
                workspace_id: crate::WorkspaceId,
                is_open: bool
            ) -> Result<()> {
                INSERT OR REPLACE INTO welcome_pages(item_id, workspace_id, is_open)
                VALUES (?, ?, ?)
            }
        }

        query! {
            pub fn get_welcome_page(
                item_id: crate::ItemId,
                workspace_id: crate::WorkspaceId
            ) -> Result<bool> {
                SELECT is_open
                FROM welcome_pages
                WHERE item_id = ? AND workspace_id = ?
            }
        }
    }
}

fn project_name(paths: &PathList) -> String {
    let joined = paths
        .paths()
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .collect::<Vec<_>>()
        .join(", ");
    if joined.is_empty() {
        "Untitled".to_string()
    } else {
        joined
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn welcome_page_is_vireo_branded_with_vs_code_start_entries() {
        assert_eq!(WELCOME_TITLE, "Vireo");
        assert!(!WELCOME_SUBTITLE.contains("Zed"));
        assert_eq!(
            START.entries.map(|entry| entry.title),
            [
                "New File...",
                "Open Folder...",
                "Clone Git Repository...",
                "Connect to...",
            ]
        );
        assert!(
            WALKTHROUGHS
                .iter()
                .all(|walkthrough| !walkthrough.title.contains("Zed"))
        );
    }

    #[gpui::test]
    async fn welcome_page_opens_while_the_workspace_is_updating(cx: &mut gpui::TestAppContext) {
        crate::tests::init_test(cx);
        let fs = fs::FakeFs::new(cx.executor());
        let project = project::Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));
        // "Show Welcome" builds the page inside a workspace update; reading
        // the workspace from the constructor used to panic there.
        workspace.update_in(cx, |workspace, window, cx| {
            let handle = workspace.weak_handle();
            let page = cx.new(|cx| WelcomePage::new(handle, false, window, cx));
            workspace.add_item_to_active_pane(Box::new(page), None, true, window, cx);
        });
        cx.run_until_parked();
    }

    #[test]
    fn recent_project_shows_its_parent_folder() {
        let paths = PathList::new(&["/srv/code/vireo"]);
        assert_eq!(recent_project_parent(&paths).as_deref(), Some("/srv/code"));
        assert_eq!(recent_project_parent(&PathList::new::<&str>(&[])), None);
    }

    #[test]
    fn test_project_name_empty() {
        let paths = PathList::new::<&str>(&[]);
        assert_eq!(project_name(&paths), "Untitled");
    }

    #[test]
    fn test_project_name_single() {
        let paths = PathList::new(&["/home/user/my-project"]);
        assert_eq!(project_name(&paths), "my-project");
    }

    #[test]
    fn test_project_name_multiple() {
        // PathList sorts lexicographically, so filenames appear in alpha order
        let paths = PathList::new(&["/home/user/zed", "/home/user/api"]);
        assert_eq!(project_name(&paths), "api, zed");
    }

    #[test]
    fn test_project_name_root_path_filtered() {
        // A bare root "/" has no file_name(), falls back to "Untitled"
        let paths = PathList::new(&["/"]);
        assert_eq!(project_name(&paths), "Untitled");
    }
}
