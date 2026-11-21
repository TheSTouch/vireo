//! The activity bar: the vertical strip of view icons at the window's left
//! edge, as in VS Code. It lists every panel docked left, so panels added by
//! extensions appear without changes here.

use gpui::{
    Action, AnyElement, App, Context, Entity, Focusable, IntoElement, ParentElement, Render,
    SharedString, Styled, Subscription, WeakEntity, Window, div, px,
};
use theme::ActiveTheme;
use ui::{
    ButtonSize, ContextMenu, Icon, IconButton, IconName, IconSize, PopoverMenu, Tooltip, prelude::*,
};
use workspace::{
    DeploySearch, Workspace,
    dock::{Dock, DockPosition},
};

pub const ACTIVITY_BAR_WIDTH: f32 = 48.;
const ACTIVITY_ICON_SIZE: f32 = 24.;

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, window, cx| {
        let Some(window) = window else {
            return;
        };
        let workspace_handle = cx.entity().downgrade();
        let left_dock = workspace.left_dock().clone();
        let activity_bar = cx.new(|cx| ActivityBar::new(workspace_handle, left_dock, window, cx));
        workspace.set_activity_bar(Some(activity_bar.into()), cx);
    })
    .detach();
}

pub struct ActivityBar {
    workspace: WeakEntity<Workspace>,
    left_dock: Entity<Dock>,
    _subscriptions: Vec<Subscription>,
}

/// One icon in the bar, in display order.
#[derive(Debug, Clone, PartialEq)]
pub enum ActivityItem {
    /// A panel in the left dock, by its index in the dock.
    Panel {
        index: usize,
        name: &'static str,
        icon: IconName,
        tooltip: SharedString,
        badge: Option<usize>,
        active: bool,
    },
    /// A VS Code view that Vireo opens as an editor tab rather than a panel.
    Search,
    Extensions,
}

impl ActivityBar {
    pub fn new(
        workspace: WeakEntity<Workspace>,
        left_dock: Entity<Dock>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = vec![cx.observe(&left_dock, |_, _, cx| cx.notify())];
        Self {
            workspace,
            left_dock,
            _subscriptions: subscriptions,
        }
    }

    /// The bar's icons in VS Code's order: Explorer, Search, Source Control,
    /// Run and Debug, Extensions, then every other left panel in dock order.
    pub fn items(&self, window: &Window, cx: &App) -> Vec<ActivityItem> {
        let dock = self.left_dock.read(cx);
        let active_index = dock.is_open().then(|| dock.active_panel_index()).flatten();
        let mut panels: Vec<ActivityItem> = dock
            .panels()
            .enumerate()
            .filter_map(|(index, panel)| {
                let panel_icon = panel.icon(window, cx)?;
                let tooltip = panel.icon_tooltip(window, cx)?;
                let name = panel.persistent_name();
                Some(ActivityItem::Panel {
                    index,
                    name,
                    icon: vs_code_icon(name).unwrap_or(panel_icon),
                    tooltip: vs_code_title(name).unwrap_or(tooltip).into(),
                    badge: panel
                        .icon_label(window, cx)
                        .and_then(|label| label.parse::<usize>().ok())
                        .filter(|count| *count > 0),
                    active: Some(index) == active_index,
                })
            })
            .collect();

        let take = |panels: &mut Vec<ActivityItem>, wanted: &str| {
            let position = panels.iter().position(
                |item| matches!(item, ActivityItem::Panel { name, .. } if *name == wanted),
            )?;
            Some(panels.remove(position))
        };
        let mut items = Vec::new();
        items.extend(take(&mut panels, "Project Panel"));
        items.push(ActivityItem::Search);
        items.extend(take(&mut panels, "GitPanel"));
        items.extend(take(&mut panels, "DebugPanel"));
        items.push(ActivityItem::Extensions);
        items.extend(panels);
        items
    }

    fn activate_panel(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let dock = self.left_dock.read(cx);
        let is_active = dock.is_open() && dock.active_panel_index() == Some(index);
        if is_active {
            // Clicking the active view collapses the sidebar, as in VS Code.
            workspace.update(cx, |workspace, cx| {
                workspace.toggle_dock(DockPosition::Left, window, cx)
            });
            return;
        }
        let panel = self.left_dock.update(cx, |dock, cx| {
            dock.activate_panel(index, window, cx);
            dock.set_open(true, window, cx);
            dock.active_panel().cloned()
        });
        if let Some(panel) = panel {
            panel.panel_focus_handle(cx).focus(window, cx);
        }
    }

    /// Runs an action that panes and the workspace handle, from the center.
    fn dispatch_from_center(&self, action: Box<dyn Action>, window: &mut Window, cx: &mut App) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let pane = workspace.read(cx).active_pane().clone();
        pane.read(cx).focus_handle(cx).focus(window, cx);
        window.dispatch_action(action, cx);
    }

    fn render_item(
        &self,
        item: ActivityItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors();
        let (id, icon, tooltip, badge, active): (SharedString, _, SharedString, _, _) = match &item
        {
            ActivityItem::Panel {
                name,
                icon,
                tooltip,
                badge,
                active,
                ..
            } => ((*name).into(), *icon, tooltip.clone(), *badge, *active),
            ActivityItem::Search => (
                "search".into(),
                IconName::ActivitySearch,
                "Search".into(),
                None,
                false,
            ),
            ActivityItem::Extensions => (
                "extensions".into(),
                IconName::ActivityExtensions,
                "Extensions".into(),
                None,
                false,
            ),
        };
        let action: Option<Box<dyn Action>> = match &item {
            ActivityItem::Panel { index, .. } => {
                let dock = self.left_dock.read(cx);
                dock.panels()
                    .nth(*index)
                    .map(|panel| panel.toggle_action(window, cx))
            }
            ActivityItem::Search => Some(DeploySearch::default().boxed_clone()),
            ActivityItem::Extensions => Some(zed_actions::Extensions::default().boxed_clone()),
        };
        let icon_color = if active {
            colors.activity_bar_foreground
        } else {
            colors.activity_bar_inactive_foreground
        };
        let hover_color = colors.activity_bar_foreground;
        let badge_colors = (
            colors.activity_bar_badge_background,
            colors.activity_bar_badge_foreground,
        );

        div()
            .id(SharedString::from(format!("activity-{id}")))
            .relative()
            .size(px(ACTIVITY_BAR_WIDTH))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .text_color(icon_color)
            .hover(move |style| style.text_color(hover_color))
            .when(active, |this| {
                this.child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(2.))
                        .bg(colors.activity_bar_active_border),
                )
            })
            .child(
                Icon::new(icon)
                    .size(IconSize::Custom(rems_from_px(ACTIVITY_ICON_SIZE)))
                    .color(Color::Custom(icon_color)),
            )
            .when_some(badge, |this, count| {
                this.child(render_badge(count, badge_colors.0, badge_colors.1))
            })
            .tooltip({
                let action = action.as_ref().map(|action| action.boxed_clone());
                move |_window, cx| match &action {
                    Some(action) => Tooltip::for_action(tooltip.clone(), &**action, cx),
                    None => Tooltip::simple(tooltip.clone(), cx),
                }
            })
            .on_click(cx.listener(move |this, _, window, cx| match &item {
                ActivityItem::Panel { index, .. } => this.activate_panel(*index, window, cx),
                ActivityItem::Search => {
                    this.dispatch_from_center(DeploySearch::default().boxed_clone(), window, cx)
                }
                ActivityItem::Extensions => this.dispatch_from_center(
                    zed_actions::Extensions::default().boxed_clone(),
                    window,
                    cx,
                ),
            }))
    }

    fn render_bottom_menus(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let inactive = colors.activity_bar_inactive_foreground;
        let menu_button = |id: &'static str, icon: IconName| {
            IconButton::new(id, icon)
                .icon_size(IconSize::Custom(rems_from_px(ACTIVITY_ICON_SIZE)))
                .icon_color(Color::Custom(inactive))
                .size(ButtonSize::Large)
        };
        let cell = |menu: AnyElement| {
            div()
                .size(px(ACTIVITY_BAR_WIDTH))
                .flex()
                .items_center()
                .justify_center()
                .child(menu)
        };
        v_flex()
            .child(cell(
                menu_button("activity-develop", IconName::ActivityDevelop)
                    .tooltip(Tooltip::text("Develop Vireo"))
                    .on_click(|_, window, cx| {
                        window.dispatch_action(zed_actions::develop_vireo::Open.boxed_clone(), cx)
                    })
                    .into_any_element(),
            ))
            .child(cell(
                PopoverMenu::new("activity-accounts")
                    .anchor(gpui::Anchor::BottomLeft)
                    .attach(gpui::Anchor::BottomRight)
                    .trigger_with_tooltip(
                        menu_button("activity-accounts-button", IconName::ActivityAccounts),
                        Tooltip::text("Accounts"),
                    )
                    .menu(|window, cx| {
                        Some(ContextMenu::build(window, cx, |menu, _, _| {
                            menu.action("Sign In", Box::new(client::SignIn)).action(
                                "Account Settings",
                                Box::new(zed_actions::OpenAccountSettings),
                            )
                        }))
                    })
                    .into_any_element(),
            ))
            .child(cell(
                PopoverMenu::new("activity-manage")
                    .anchor(gpui::Anchor::BottomLeft)
                    .attach(gpui::Anchor::BottomRight)
                    .trigger_with_tooltip(
                        menu_button("activity-manage-button", IconName::ActivityManage),
                        Tooltip::text("Manage"),
                    )
                    .menu(|window, cx| {
                        Some(ContextMenu::build(window, cx, |menu, _, _| {
                            menu.action(
                                "Command Palette...",
                                Box::new(zed_actions::command_palette::Toggle),
                            )
                            .separator()
                            .action("Settings", Box::new(zed_actions::OpenSettings))
                            .action(
                                "Extensions",
                                zed_actions::Extensions::default().boxed_clone(),
                            )
                            .action("Keyboard Shortcuts", Box::new(zed_actions::OpenKeymap))
                            .separator()
                            .action(
                                "Color Theme",
                                zed_actions::theme_selector::Toggle::default().boxed_clone(),
                            )
                            .action(
                                "File Icon Theme",
                                zed_actions::icon_theme_selector::Toggle::default().boxed_clone(),
                            )
                        }))
                    })
                    .into_any_element(),
            ))
    }
}

fn render_badge(count: usize, background: gpui::Hsla, foreground: gpui::Hsla) -> impl IntoElement {
    let label: SharedString = if count > 999 {
        "999+".into()
    } else {
        count.to_string().into()
    };
    div()
        .absolute()
        .right(px(6.))
        .bottom(px(8.))
        .min_w(px(16.))
        .h(px(16.))
        .px(px(3.))
        .rounded_full()
        .bg(background)
        .text_color(foreground)
        .text_size(px(9.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .flex()
        .items_center()
        .justify_center()
        .child(label)
}

/// VS Code's activity bar icons for the views Zed provides as panels.
fn vs_code_icon(panel_name: &str) -> Option<IconName> {
    match panel_name {
        "Project Panel" => Some(IconName::ActivityExplorer),
        "GitPanel" => Some(IconName::ActivitySourceControl),
        "DebugPanel" => Some(IconName::ActivityRunDebug),
        "Outline Panel" => Some(IconName::Outline),
        _ => None,
    }
}

/// VS Code's names for those views.
fn vs_code_title(panel_name: &str) -> Option<&'static str> {
    match panel_name {
        "Project Panel" => Some("Explorer"),
        "GitPanel" => Some("Source Control"),
        "DebugPanel" => Some("Run and Debug"),
        "Outline Panel" => Some("Outline"),
        _ => None,
    }
}

impl Render for ActivityBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let items = self.items(window, cx);
        v_flex()
            .id("activity-bar")
            .h_full()
            .w(px(ACTIVITY_BAR_WIDTH))
            .flex_none()
            .bg(colors.activity_bar_background)
            .border_r_1()
            .border_color(colors.border)
            .justify_between()
            .child(
                v_flex().children(
                    items
                        .into_iter()
                        .map(|item| self.render_item(item, window, cx).into_any_element()),
                ),
            )
            .child(self.render_bottom_menus(cx))
    }
}

#[cfg(test)]
mod activity_bar_tests;
