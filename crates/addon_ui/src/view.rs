use std::str::FromStr as _;
use std::sync::Arc;

use collections::{HashMap, HashSet};
use extension::{AddonTreeItem, AddonViewKind};
use gpui::{
    App, Context, DismissEvent, Entity, FocusHandle, Focusable, IntoElement, MouseDownEvent,
    Pixels, Point, Render, Subscription, Window, anchored, deferred,
};
use ui::{ContextMenu, IconName, ListItem, ListItemSpacing, prelude::*};

use crate::{AddonRegistry, AddonRegistryEvent, AddonViewKey, RunCommand};

/// The content of an addon view, shown in a panel or an editor tab.
pub struct AddonView {
    key: AddonViewKey,
    kind: AddonViewKind,
    registry: Entity<AddonRegistry>,
    focus_handle: FocusHandle,
    tree: TreeState,
    error: Option<SharedString>,
    context_menu: Option<(Entity<ContextMenu>, Point<Pixels>, Subscription)>,
    _registry_subscription: Subscription,
}

#[derive(Default)]
struct TreeState {
    /// Children by parent item ID; `None` is the root.
    children: HashMap<Option<Arc<str>>, Vec<AddonTreeItem>>,
    expanded: HashSet<Arc<str>>,
    loading: HashSet<Option<Arc<str>>>,
}

impl AddonView {
    pub fn new(
        key: AddonViewKey,
        kind: AddonViewKind,
        registry: Entity<AddonRegistry>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscription = cx.subscribe_in(&registry, window, |this, _, event, window, cx| {
            let AddonRegistryEvent::RefreshView(key) = event;
            if *key == this.key {
                this.refresh(window, cx);
            }
        });
        let mut this = Self {
            key,
            kind,
            registry,
            focus_handle: cx.focus_handle(),
            tree: TreeState::default(),
            error: None,
            context_menu: None,
            _registry_subscription: subscription,
        };
        this.refresh(window, cx);
        this
    }

    pub fn key(&self) -> &AddonViewKey {
        &self.key
    }

    /// Drops everything cached and asks the addon again.
    pub fn refresh(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.error = None;
        match self.kind {
            AddonViewKind::Tree => {
                let expanded = self.tree.expanded.clone();
                self.tree.children.clear();
                self.load_children(None, cx);
                for parent in expanded {
                    self.load_children(Some(parent), cx);
                }
            }
        }
        cx.notify();
    }

    fn load_children(&mut self, parent: Option<Arc<str>>, cx: &mut Context<Self>) {
        if !self.tree.loading.insert(parent.clone()) {
            return;
        }
        let task = self
            .registry
            .read(cx)
            .tree_children(&self.key, parent.clone(), cx);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.tree.loading.remove(&parent);
                match result {
                    Ok(items) => {
                        this.tree.children.insert(parent, items);
                    }
                    Err(error) => {
                        log::error!("addon tree view {:?} failed: {error:#}", this.key);
                        this.error = Some(format!("{error:#}").into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The items currently shown, in display order, with their depth.
    pub fn visible_items(&self) -> Vec<(usize, AddonTreeItem)> {
        let mut items = Vec::new();
        self.collect_visible(None, 0, &mut items);
        items
    }

    fn collect_visible(
        &self,
        parent: Option<Arc<str>>,
        depth: usize,
        items: &mut Vec<(usize, AddonTreeItem)>,
    ) {
        let Some(children) = self.tree.children.get(&parent) else {
            return;
        };
        for item in children {
            items.push((depth, item.clone()));
            let item_id: Arc<str> = item.id.as_str().into();
            if self.tree.expanded.contains(&item_id) {
                self.collect_visible(Some(item_id), depth + 1, items);
            }
        }
    }

    pub fn toggle_expanded(&mut self, item_id: &str, cx: &mut Context<Self>) {
        let item_id: Arc<str> = item_id.into();
        if !self.tree.expanded.remove(&item_id) {
            self.tree.expanded.insert(item_id.clone());
            if !self.tree.children.contains_key(&Some(item_id.clone())) {
                self.load_children(Some(item_id), cx);
            }
        }
        cx.notify();
    }

    fn deploy_context_menu(
        &mut self,
        item: &AddonTreeItem,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entries = self
            .registry
            .read(cx)
            .context_menu_entries(&self.key, item.context_value.as_deref());
        if entries.is_empty() {
            return;
        }
        let extension_id = self.key.extension_id.clone();
        let item_id = item.id.clone();
        let menu = ContextMenu::build(window, cx, move |mut menu, _, _| {
            for (command_id, title) in entries {
                menu = menu.action(
                    title,
                    Box::new(RunCommand {
                        extension: extension_id.to_string(),
                        command: command_id.to_string(),
                        args: vec![item_id.clone()],
                    }),
                );
            }
            menu
        });
        let subscription = cx.subscribe(&menu, |this, _, _: &DismissEvent, cx| {
            this.context_menu.take();
            cx.notify();
        });
        window.focus(&menu.focus_handle(cx), cx);
        self.context_menu = Some((menu, position, subscription));
        cx.notify();
    }

    fn render_tree_item(
        &self,
        depth: usize,
        item: AddonTreeItem,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let item_id: Arc<str> = item.id.as_str().into();
        let expanded = self.tree.expanded.contains(&item_id);
        let icon = item
            .icon
            .as_deref()
            .and_then(|icon| IconName::from_str(icon).ok());
        ListItem::new(SharedString::from(format!("addon-tree-{}", item.id)))
            .indent_level(depth)
            .indent_step_size(px(12.))
            .spacing(ListItemSpacing::Sparse)
            .when(item.collapsible, |row| row.toggle(expanded))
            .when_some(icon, |row, icon| {
                row.start_slot(Icon::new(icon).size(IconSize::Small).color(Color::Muted))
            })
            .child(
                h_flex()
                    .gap_2()
                    .child(Label::new(item.label.clone()))
                    .when_some(item.description.clone(), |row, description| {
                        row.child(
                            Label::new(description)
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                    }),
            )
            .on_click(cx.listener({
                let item = item.clone();
                move |this, _, window, cx| {
                    if item.collapsible {
                        this.toggle_expanded(&item.id, cx);
                    }
                    if let Some(command) = &item.command {
                        window.dispatch_action(
                            Box::new(RunCommand {
                                extension: this.key.extension_id.to_string(),
                                command: command.clone(),
                                args: vec![item.id.clone()],
                            }),
                            cx,
                        );
                    }
                }
            }))
            .on_secondary_mouse_down(cx.listener(
                move |this, event: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.deploy_context_menu(&item, event.position, window, cx);
                },
            ))
            .into_any_element()
    }
}

impl Focusable for AddonView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for AddonView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows: Vec<AnyElement> = match self.kind {
            AddonViewKind::Tree => self
                .visible_items()
                .into_iter()
                .map(|(depth, item)| self.render_tree_item(depth, item, cx))
                .collect(),
        };
        v_flex()
            .id(SharedString::from(format!(
                "addon-view-{}-{}",
                self.key.extension_id, self.key.view_id
            )))
            .track_focus(&self.focus_handle)
            .size_full()
            .overflow_y_scroll()
            .children(rows)
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .px_2()
                        .child(Label::new(error).color(Color::Error).size(LabelSize::Small)),
                )
            })
            .children(self.context_menu.as_ref().map(|(menu, position, _)| {
                deferred(anchored().position(*position).child(menu.clone())).with_priority(3)
            }))
    }
}
