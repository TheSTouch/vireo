use std::cmp::Ordering;

use gpui::{AnyElement, IntoElement, Stateful};
use smallvec::SmallVec;

use crate::prelude::*;

const START_TAB_SLOT_SIZE: Pixels = px(12.);
/// VS Code's close button sits in a 20 px square at the tab's end.
const END_TAB_SLOT_SIZE: Pixels = px(20.);
/// VS Code's tabs are 35 px tall whatever the UI font size.
const VS_CODE_TAB_HEIGHT: f32 = 35.;
/// VS Code pads a tab's label 10 px from its start.
const TAB_START_PADDING: Pixels = px(10.);
const TAB_END_PADDING: Pixels = px(4.);

/// The position of a [`Tab`] within a list of tabs.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TabPosition {
    /// The tab is first in the list.
    First,

    /// The tab is in the middle of the list (i.e., it is not the first or last tab).
    ///
    /// The [`Ordering`] is where this tab is positioned with respect to the selected tab.
    Middle(Ordering),

    /// The tab is last in the list.
    Last,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TabCloseSide {
    Start,
    End,
}

#[derive(IntoElement, RegisterComponent)]
pub struct Tab {
    div: Stateful<Div>,
    selected: bool,
    position: TabPosition,
    close_side: TabCloseSide,
    start_slot: Option<AnyElement>,
    end_slot: Option<AnyElement>,
    children: SmallVec<[AnyElement; 2]>,
}

impl Tab {
    pub fn new(id: impl Into<ElementId>) -> Self {
        let id = id.into();
        Self {
            div: div()
                .id(id.clone())
                .debug_selector(|| format!("TAB-{}", id)),
            selected: false,
            position: TabPosition::First,
            close_side: TabCloseSide::End,
            start_slot: None,
            end_slot: None,
            children: SmallVec::new(),
        }
    }

    pub fn position(mut self, position: TabPosition) -> Self {
        self.position = position;
        self
    }

    pub fn close_side(mut self, close_side: TabCloseSide) -> Self {
        self.close_side = close_side;
        self
    }

    pub fn start_slot<E: IntoElement>(mut self, element: impl Into<Option<E>>) -> Self {
        self.start_slot = element.into().map(IntoElement::into_any_element);
        self
    }

    pub fn end_slot<E: IntoElement>(mut self, element: impl Into<Option<E>>) -> Self {
        self.end_slot = element.into().map(IntoElement::into_any_element);
        self
    }

    pub fn content_height(cx: &App) -> Pixels {
        Self::container_height(cx) - px(1.)
    }

    /// VS Code's 35 px, scaled only by the UI density setting: Zed's
    /// spacing also grows with the UI font size, VS Code's tabs don't.
    pub fn container_height(cx: &App) -> Pixels {
        // Base32 is 2 rems at the default density.
        let density = DynamicSpacing::Base32.rems(cx).0 / 2.;
        px((VS_CODE_TAB_HEIGHT * density).round())
    }
}

impl InteractiveElement for Tab {
    fn interactivity(&mut self) -> &mut gpui::Interactivity {
        self.div.interactivity()
    }
}

impl StatefulInteractiveElement for Tab {}

impl Toggleable for Tab {
    fn toggle_state(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
}

impl ParentElement for Tab {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements)
    }
}

impl RenderOnce for Tab {
    #[allow(refining_impl_trait)]
    fn render(self, _: &mut Window, cx: &mut App) -> Stateful<Div> {
        let colors = cx.theme().colors();
        let (text_color, tab_bg) = match self.selected {
            false => (
                colors.tab_inactive_foreground,
                colors.tab_inactive_background,
            ),
            true => (colors.tab_active_foreground, colors.tab_active_background),
        };
        let active_border = colors.tab_active_border;

        // An empty start slot takes no room, so the label starts 10 px in as in
        // VS Code; the end slot always keeps room for the close button.
        let (start_slot, end_slot) = {
            let start_slot = self.start_slot.map(|start_slot| {
                h_flex()
                    .size(START_TAB_SLOT_SIZE)
                    .justify_center()
                    .child(start_slot)
            });

            let end_slot = Some(
                h_flex()
                    .size(END_TAB_SLOT_SIZE)
                    .justify_center()
                    .children(self.end_slot),
            );

            match self.close_side {
                TabCloseSide::End => (start_slot, end_slot),
                TabCloseSide::Start => (end_slot, start_slot),
            }
        };

        // VS Code: every tab has a border on its trailing side; inactive tabs
        // also draw the tab bar's bottom border, which the active tab covers.
        // The active tab carries a 1 px accent line along its top.
        self.div
            .relative()
            .h(Tab::container_height(cx))
            .bg(tab_bg)
            .border_color(colors.border)
            .border_r_1()
            .map(|this| {
                if self.selected {
                    this.pb_px()
                } else {
                    this.border_b_1()
                }
            })
            .when(
                self.selected && matches!(self.position, TabPosition::First),
                |this| this.pl_px(),
            )
            .cursor_pointer()
            .when(self.selected, |this| {
                this.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h_px()
                        .bg(active_border),
                )
            })
            .child(
                h_flex()
                    .group("")
                    .relative()
                    .h(Tab::content_height(cx))
                    .pl(TAB_START_PADDING)
                    .pr(TAB_END_PADDING)
                    .gap(DynamicSpacing::Base04.rems(cx))
                    .text_color(text_color)
                    .children(start_slot)
                    .children(self.children)
                    .children(end_slot),
            )
    }
}

impl Component for Tab {
    fn scope() -> ComponentScope {
        ComponentScope::Navigation
    }

    fn description() -> &'static str {
        "A tab component that can be used in a tabbed interface, \
        supporting different positions and states."
    }

    fn preview(_window: &mut Window, _cx: &mut App) -> AnyElement {
        v_flex()
            .gap_6()
            .children(vec![example_group_with_title(
                "Variations",
                vec![
                    single_example(
                        "Default",
                        Tab::new("default").child("Default Tab").into_any_element(),
                    ),
                    single_example(
                        "Selected",
                        Tab::new("selected")
                            .toggle_state(true)
                            .child("Selected Tab")
                            .into_any_element(),
                    ),
                    single_example(
                        "First",
                        Tab::new("first")
                            .position(TabPosition::First)
                            .child("First Tab")
                            .into_any_element(),
                    ),
                    single_example(
                        "Middle",
                        Tab::new("middle")
                            .position(TabPosition::Middle(Ordering::Equal))
                            .child("Middle Tab")
                            .into_any_element(),
                    ),
                    single_example(
                        "Last",
                        Tab::new("last")
                            .position(TabPosition::Last)
                            .child("Last Tab")
                            .into_any_element(),
                    ),
                ],
            )])
            .into_any_element()
    }
}
