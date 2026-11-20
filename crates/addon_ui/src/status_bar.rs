use std::sync::Arc;

use gpui::{App, Context, Entity, IntoElement, Render, Subscription, Window};
use ui::{Button, LabelSize, Tooltip, prelude::*};
use workspace::{HideStatusItem, ItemHandle, StatusItemView, Workspace};

use crate::{AddonRegistry, RunCommand};

/// The status bar items of every addon, shown together on the right of the status bar.
pub struct AddonStatusItems {
    registry: Entity<AddonRegistry>,
    _registry_subscription: Subscription,
}

impl AddonStatusItems {
    pub fn new(registry: Entity<AddonRegistry>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&registry, |_, _, cx| cx.notify());
        Self {
            registry,
            _registry_subscription: subscription,
        }
    }
}

impl Render for AddonStatusItems {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let items = self.registry.read(cx).status_items();
        h_flex()
            .gap_1()
            .children(items.into_iter().map(|(extension_id, item)| {
                let element_id = format!("addon-status-{extension_id}-{}", item.id);
                Button::new(SharedString::from(element_id), item.text.clone())
                    .label_size(LabelSize::Small)
                    .when_some(item.tooltip, |button, tooltip| {
                        button.tooltip(Tooltip::text(tooltip))
                    })
                    .when_some(item.command, |button, command| {
                        let extension_id: Arc<str> = extension_id.clone();
                        button.on_click(move |_, window, cx| {
                            window.dispatch_action(
                                Box::new(RunCommand {
                                    extension: extension_id.to_string(),
                                    command: command.to_string(),
                                    args: Vec::new(),
                                }),
                                cx,
                            );
                        })
                    })
            }))
    }
}

impl StatusItemView for AddonStatusItems {
    fn set_active_pane_item(
        &mut self,
        _active_pane_item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    /// Addon items appear only while an addon shows them, so there is nothing to hide.
    fn hide_setting(&self, _cx: &App) -> Option<HideStatusItem> {
        None
    }
}

pub(crate) fn add_to_workspace(
    workspace: &mut Workspace,
    registry: Entity<AddonRegistry>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let items = cx.new(|cx| AddonStatusItems::new(registry, cx));
    workspace.status_bar().update(cx, |status_bar, cx| {
        status_bar.add_right_item(items, window, cx);
    });
}
