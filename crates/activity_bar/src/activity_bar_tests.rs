use fs::FakeFs;
use gpui::{
    Action, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, Pixels,
    Render, TestAppContext, VisualTestContext, Window, actions, div, px,
};
use project::Project;
use settings::SettingsStore;
use ui::{IconName, IntoElement};
use workspace::{
    Workspace,
    dock::{DockPosition, Panel, PanelEvent},
};

use crate::{ActivityBar, ActivityItem};

actions!(activity_bar_test, [ToggleTestPanel]);

macro_rules! test_panel {
    ($type:ident, $name:literal, $icon:expr, $label:expr, $priority:literal) => {
        struct $type {
            focus_handle: FocusHandle,
        }

        impl $type {
            fn new(cx: &mut Context<Self>) -> Self {
                Self {
                    focus_handle: cx.focus_handle(),
                }
            }
        }

        impl EventEmitter<PanelEvent> for $type {}

        impl Focusable for $type {
            fn focus_handle(&self, _cx: &App) -> FocusHandle {
                self.focus_handle.clone()
            }
        }

        impl Render for $type {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div()
            }
        }

        impl Panel for $type {
            fn persistent_name() -> &'static str {
                $name
            }
            fn panel_key() -> &'static str {
                $name
            }
            fn position(&self, _: &Window, _: &App) -> DockPosition {
                DockPosition::Left
            }
            fn position_is_valid(&self, position: DockPosition) -> bool {
                position == DockPosition::Left
            }
            fn set_position(&mut self, _: DockPosition, _: &mut Window, _: &mut Context<Self>) {}
            fn default_size(&self, _: &Window, _: &App) -> Pixels {
                px(300.)
            }
            fn icon(&self, _: &Window, _: &App) -> Option<IconName> {
                Some($icon)
            }
            fn icon_tooltip(&self, _: &Window, _: &App) -> Option<&'static str> {
                Some($name)
            }
            fn toggle_action(&self) -> Box<dyn Action> {
                Box::new(ToggleTestPanel)
            }
            fn icon_label(&self, _: &Window, _: &App) -> Option<String> {
                $label
            }
            fn activation_priority(&self) -> u32 {
                $priority
            }
        }
    };
}

test_panel!(ExplorerPanel, "Project Panel", IconName::FileTree, None, 1);
test_panel!(
    SourceControlPanel,
    "GitPanel",
    IconName::GitBranch,
    Some("3".to_string()),
    2
);
test_panel!(AddonPanel, "Addon Panel", IconName::Blocks, None, 3);

fn init_test(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let settings_store = SettingsStore::test(cx);
        cx.set_global(settings_store);
        theme_settings::init(theme::LoadThemes::JustBase, cx);
    });
}

async fn workspace_with_panels(
    cx: &mut TestAppContext,
) -> (
    Entity<Workspace>,
    Entity<ActivityBar>,
    &mut VisualTestContext,
) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    let project = Project::test(fs, [], cx).await;
    let (workspace, cx) = cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));
    let activity_bar = workspace.update_in(cx, |workspace, window, cx| {
        // Added out of VS Code's order, to check the bar sorts them.
        let addon = cx.new(AddonPanel::new);
        workspace.add_panel(addon, window, cx);
        let source_control = cx.new(SourceControlPanel::new);
        workspace.add_panel(source_control, window, cx);
        let explorer = cx.new(ExplorerPanel::new);
        workspace.add_panel(explorer, window, cx);
        let left_dock = workspace.left_dock().clone();
        let handle = cx.entity().downgrade();
        cx.new(|cx| ActivityBar::new(handle, left_dock, window, cx))
    });
    (workspace, activity_bar, cx)
}

fn panel_names(items: &[ActivityItem]) -> Vec<String> {
    items
        .iter()
        .map(|item| match item {
            ActivityItem::Panel { tooltip, .. } => tooltip.to_string(),
            ActivityItem::Search => "Search".to_string(),
            ActivityItem::Extensions => "Extensions".to_string(),
        })
        .collect()
}

#[gpui::test]
async fn lists_left_panels_in_vs_code_order(cx: &mut TestAppContext) {
    let (_workspace, activity_bar, cx) = workspace_with_panels(cx).await;
    let items = activity_bar.update_in(cx, |bar, window, cx| bar.items(window, cx));
    assert_eq!(
        panel_names(&items),
        [
            "Explorer",
            "Search",
            "Source Control",
            "Extensions",
            "Addon Panel"
        ]
    );
    let badges: Vec<_> = items
        .iter()
        .filter_map(|item| match item {
            ActivityItem::Panel { badge, .. } => Some(*badge),
            _ => None,
        })
        .collect();
    assert_eq!(badges, [None, Some(3), None]);
    assert!(
        items.iter().any(|item| matches!(
            item,
            ActivityItem::Panel {
                icon: IconName::ActivityExplorer,
                ..
            }
        )),
        "the project panel should use VS Code's Explorer icon"
    );
}

#[gpui::test]
async fn clicking_the_active_view_collapses_the_sidebar(cx: &mut TestAppContext) {
    let (workspace, activity_bar, cx) = workspace_with_panels(cx).await;
    let explorer_index = workspace.read_with(cx, |workspace, cx| {
        workspace
            .left_dock()
            .read(cx)
            .panel_index_for_type::<ExplorerPanel>()
            .expect("explorer should be docked left")
    });

    activity_bar.update_in(cx, |bar, window, cx| {
        bar.activate_panel(explorer_index, window, cx)
    });
    cx.run_until_parked();
    workspace.read_with(cx, |workspace, cx| {
        let dock = workspace.left_dock().read(cx);
        assert!(dock.is_open());
        assert_eq!(dock.active_panel_index(), Some(explorer_index));
    });
    let items = activity_bar.update_in(cx, |bar, window, cx| bar.items(window, cx));
    assert!(matches!(items[0], ActivityItem::Panel { active: true, .. }));

    activity_bar.update_in(cx, |bar, window, cx| {
        bar.activate_panel(explorer_index, window, cx)
    });
    cx.run_until_parked();
    workspace.read_with(cx, |workspace, cx| {
        assert!(!workspace.left_dock().read(cx).is_open());
    });
}

#[gpui::test]
async fn the_editor_area_fills_the_window_beside_the_bar(cx: &mut TestAppContext) {
    let (workspace, activity_bar, cx) = workspace_with_panels(cx).await;
    cx.simulate_resize(gpui::size(px(800.), px(600.)));
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.set_activity_bar(Some(activity_bar.into()), cx);
        let item = cx.new(|cx| workspace::item::test::TestItem::new(cx).with_label("main.rs"));
        workspace.add_item_to_active_pane(Box::new(item), None, true, window, cx);
    });
    cx.run_until_parked();

    // The row holding the bar and the workspace area once centered its
    // children, which collapsed the editor area to a line mid-window.
    let tab = cx.debug_bounds("TAB-0").expect("the tab should be drawn");
    assert!(
        tab.top() < px(50.),
        "the tab bar should sit at the top of the window, not at {:?}",
        tab.top()
    );
    assert!(
        tab.left() >= px(48.),
        "the editor area should start after the 48 px activity bar, not at {:?}",
        tab.left()
    );
}
