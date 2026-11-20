//! Vireo's VS Code-style title bar controls: the command center in the middle
//! (back, forward and a search box that opens Quick Open) and the layout
//! toggles on the right (primary side bar, panel, secondary side bar).

use gpui::{Action, App, Div, Hsla, IntoElement, ParentElement, Stateful, Styled, WeakEntity};
use ui::{IconButton, IconName, IconSize, Tooltip, prelude::*};
use workspace::{
    GoBack, GoForward, ToggleBottomDock, ToggleFileFinder, ToggleLeftDock, ToggleRightDock,
    Workspace, dock::DockPosition,
};

/// VS Code's search box is at most 600 px wide and takes 38% of the bar; the
/// back and forward buttons add about 50 px beside it.
const COMMAND_CENTER_WIDTH: f32 = 0.42;
const COMMAND_CENTER_MAX_WIDTH: f32 = 650.;
const COMMAND_CENTER_MIN_WIDTH: f32 = 220.;
const COMMAND_CENTER_HEIGHT: f32 = 22.;

/// The text in the command center box: the project's name, or "Search" when
/// no folder is open, as in VS Code.
pub(crate) fn command_center_label(project_name: Option<&str>) -> SharedString {
    match project_name {
        Some(name) if !name.is_empty() => SharedString::from(name.to_string()),
        _ => SharedString::new_static("Search"),
    }
}

/// VS Code's workbench colors for the command center, derived from the title
/// bar's text color so they follow any theme: 5% fill, 20% border.
pub(crate) fn command_center_colors(foreground: Hsla) -> (Hsla, Hsla) {
    (foreground.opacity(0.05), foreground.opacity(0.2))
}

pub(crate) fn render_command_center(project_name: Option<SharedString>, cx: &App) -> Stateful<Div> {
    let foreground = cx.theme().colors().text;
    let (background, border) = command_center_colors(foreground);
    let label = command_center_label(project_name.as_deref());
    let tooltip_label = format!("Search {label}");

    h_flex()
        .id("command-center")
        .gap_1()
        .flex_none()
        // Relative to the title bar, since this is one of its direct children.
        .w(relative(COMMAND_CENTER_WIDTH))
        .max_w(px(COMMAND_CENTER_MAX_WIDTH))
        .min_w(px(COMMAND_CENTER_MIN_WIDTH))
        .child(
            IconButton::new("command-center-back", IconName::ArrowLeft)
                .icon_size(IconSize::Small)
                .tooltip(|_window, cx| Tooltip::for_action("Go Back", &GoBack, cx))
                .on_click(|_, window, cx| window.dispatch_action(GoBack.boxed_clone(), cx)),
        )
        .child(
            IconButton::new("command-center-forward", IconName::ArrowRight)
                .icon_size(IconSize::Small)
                .tooltip(|_window, cx| Tooltip::for_action("Go Forward", &GoForward, cx))
                .on_click(|_, window, cx| window.dispatch_action(GoForward.boxed_clone(), cx)),
        )
        .child(
            h_flex()
                .id("command-center-search")
                .h(px(COMMAND_CENTER_HEIGHT))
                .flex_1()
                .min_w_0()
                .px_2()
                .gap_1()
                .justify_center()
                .rounded(px(6.))
                .border_1()
                .border_color(border)
                .bg(background)
                .cursor_pointer()
                .hover(move |style| style.bg(foreground.opacity(0.08)))
                .child(
                    Icon::new(IconName::MagnifyingGlass)
                        .size(IconSize::XSmall)
                        .color(Color::Muted),
                )
                .child(
                    Label::new(label)
                        .size(LabelSize::Small)
                        .color(Color::Muted)
                        .truncate(),
                )
                .tooltip(move |_window, cx| {
                    Tooltip::for_action(tooltip_label.clone(), &ToggleFileFinder::default(), cx)
                })
                .on_click(|_, window, cx| {
                    window.dispatch_action(ToggleFileFinder::default().boxed_clone(), cx)
                }),
        )
}

/// Which of the three layout areas are showing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct LayoutState {
    pub primary_side_bar: bool,
    pub panel: bool,
    pub secondary_side_bar: bool,
}

impl LayoutState {
    pub(crate) fn of(workspace: &Workspace, cx: &App) -> Self {
        let open = |position| workspace.dock_at_position(position).read(cx).is_open();
        Self {
            primary_side_bar: open(DockPosition::Left),
            panel: open(DockPosition::Bottom),
            secondary_side_bar: open(DockPosition::Right),
        }
    }
}

/// One toggle in the title bar's layout controls, in VS Code's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LayoutToggle {
    PrimarySideBar,
    Panel,
    SecondarySideBar,
}

impl LayoutToggle {
    pub(crate) const ALL: [Self; 3] = [Self::PrimarySideBar, Self::Panel, Self::SecondarySideBar];

    /// VS Code draws the area filled while it shows and outlined ("off")
    /// while it is hidden.
    pub(crate) fn icon(self, state: LayoutState) -> IconName {
        match (self, self.is_open(state)) {
            (Self::PrimarySideBar, true) => IconName::LayoutSidebarLeft,
            (Self::PrimarySideBar, false) => IconName::LayoutSidebarLeftOff,
            (Self::Panel, true) => IconName::LayoutPanel,
            (Self::Panel, false) => IconName::LayoutPanelOff,
            (Self::SecondarySideBar, true) => IconName::LayoutSidebarRight,
            (Self::SecondarySideBar, false) => IconName::LayoutSidebarRightOff,
        }
    }

    pub(crate) fn is_open(self, state: LayoutState) -> bool {
        match self {
            Self::PrimarySideBar => state.primary_side_bar,
            Self::Panel => state.panel,
            Self::SecondarySideBar => state.secondary_side_bar,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::PrimarySideBar => "Toggle Primary Side Bar",
            Self::Panel => "Toggle Panel",
            Self::SecondarySideBar => "Toggle Secondary Side Bar",
        }
    }

    fn action(self) -> Box<dyn Action> {
        match self {
            Self::PrimarySideBar => ToggleLeftDock.boxed_clone(),
            Self::Panel => ToggleBottomDock.boxed_clone(),
            Self::SecondarySideBar => ToggleRightDock.boxed_clone(),
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::PrimarySideBar => "layout-toggle-primary-side-bar",
            Self::Panel => "layout-toggle-panel",
            Self::SecondarySideBar => "layout-toggle-secondary-side-bar",
        }
    }
}

pub(crate) fn render_layout_controls(
    workspace: &WeakEntity<Workspace>,
    cx: &App,
) -> Option<impl IntoElement> {
    let state = workspace
        .upgrade()
        .map(|workspace| LayoutState::of(workspace.read(cx), cx))?;
    Some(h_flex().gap_0p5().children(LayoutToggle::ALL.map(|toggle| {
        let action = toggle.action();
        let tooltip_action = action.boxed_clone();
        IconButton::new(toggle.id(), toggle.icon(state))
            .icon_size(IconSize::Small)
            .tooltip(move |_window, cx| {
                Tooltip::for_action(toggle.label(), tooltip_action.as_ref(), cx)
            })
            .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_center_shows_the_project_or_search() {
        assert_eq!(command_center_label(Some("vireo")), "vireo");
        assert_eq!(command_center_label(Some("")), "Search");
        assert_eq!(command_center_label(None), "Search");
    }

    #[test]
    fn layout_toggles_follow_vs_code_order_and_icons() {
        let state = LayoutState {
            primary_side_bar: true,
            panel: false,
            secondary_side_bar: false,
        };
        let icons = LayoutToggle::ALL.map(|toggle| toggle.icon(state));
        assert_eq!(
            icons,
            [
                IconName::LayoutSidebarLeft,
                IconName::LayoutPanelOff,
                IconName::LayoutSidebarRightOff,
            ]
        );
        let all_open = LayoutState {
            primary_side_bar: true,
            panel: true,
            secondary_side_bar: true,
        };
        assert_eq!(
            LayoutToggle::ALL.map(|toggle| toggle.icon(all_open)),
            [
                IconName::LayoutSidebarLeft,
                IconName::LayoutPanel,
                IconName::LayoutSidebarRight,
            ]
        );
    }

    #[test]
    fn command_center_colors_are_translucent_title_text() {
        let text = gpui::hsla(0., 0., 0.8, 1.);
        let (background, border) = command_center_colors(text);
        assert_eq!(background.a, 0.05);
        assert_eq!(border.a, 0.2);
        assert_eq!(background.l, text.l);
    }
}
