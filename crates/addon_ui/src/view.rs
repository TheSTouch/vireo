use std::str::FromStr as _;
use std::sync::Arc;

use collections::{HashMap, HashSet};
use editor::{Editor, EditorEvent};
use extension::{AddonElementKind, AddonTreeItem, AddonViewEvent, AddonViewKind, AddonViewTree};
use gpui::{
    App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    MouseDownEvent, Pixels, Point, Render, Subscription, Window, anchored, deferred,
};
use ui::{
    Checkbox, ContextMenu, Divider, IconName, ListItem, ListItemSpacing, ToggleState, prelude::*,
};
use workspace::{Item, Workspace};

use crate::{AddonRegistry, AddonRegistryEvent, AddonViewKey, RunCommand};

/// The content of an addon view, shown in a panel or an editor tab.
pub struct AddonView {
    key: AddonViewKey,
    kind: AddonViewKind,
    registry: Entity<AddonRegistry>,
    focus_handle: FocusHandle,
    tree: TreeState,
    custom: CustomState,
    error: Option<SharedString>,
    context_menu: Option<(Entity<ContextMenu>, Point<Pixels>, Subscription)>,
    _registry_subscription: Subscription,
}

/// How deeply custom view elements may nest. Deeper elements are not shown,
/// which also stops cycles in a malformed view tree.
const MAX_ELEMENT_DEPTH: usize = 64;

#[derive(Default)]
struct CustomState {
    tree: Option<AddonViewTree>,
    /// The editors of text inputs, by element ID.
    text_inputs: HashMap<String, (Entity<Editor>, Subscription)>,
    /// The value each text input last took from the addon. Editors report
    /// edits after the fact, so an edit that yields this value is the view's
    /// own and is not sent back to the addon.
    input_values: HashMap<String, String>,
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
            custom: CustomState::default(),
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
    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
            AddonViewKind::Custom => self.render_custom(window, cx),
        }
        cx.notify();
    }

    fn render_custom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let task = self.registry.read(cx).render_view(&self.key, cx);
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(tree) => this.apply_view_tree(tree, window, cx),
                    Err(error) => {
                        log::error!("addon view {:?} failed: {error:#}", this.key);
                        this.error = Some(format!("{error:#}").into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The content of a custom view, as last rendered by the addon.
    pub fn view_tree(&self) -> Option<&AddonViewTree> {
        self.custom.tree.as_ref()
    }

    /// The editor of a custom view's text input.
    pub fn text_input(&self, element_id: &str) -> Option<Entity<Editor>> {
        self.custom
            .text_inputs
            .get(element_id)
            .map(|(editor, _)| editor.clone())
    }

    fn apply_view_tree(
        &mut self,
        tree: AddonViewTree,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut text_inputs = std::mem::take(&mut self.custom.text_inputs);
        for element in &tree.elements {
            let AddonElementKind::TextInput { value, placeholder } = &element.kind else {
                continue;
            };
            let (editor, _) = text_inputs
                .entry(element.id.clone())
                .or_insert_with(|| self.new_text_input(element.id.clone(), window, cx));
            let editor = editor.clone();
            editor.update(cx, |editor, cx| {
                editor.set_placeholder_text(placeholder.as_deref().unwrap_or(""), window, cx);
                // Keep what the user is typing; only replace the text of inputs
                // they are not editing.
                if !editor.focus_handle(cx).is_focused(window) && editor.text(cx) != *value {
                    self.custom
                        .input_values
                        .insert(element.id.clone(), value.clone());
                    editor.set_text(value.as_str(), window, cx);
                }
            });
        }
        text_inputs.retain(|id, _| {
            tree.elements.iter().any(|element| {
                element.id == *id && matches!(element.kind, AddonElementKind::TextInput { .. })
            })
        });
        self.custom
            .input_values
            .retain(|id, _| text_inputs.contains_key(id));
        self.custom.text_inputs = text_inputs;
        self.custom.tree = Some(tree);
    }

    fn new_text_input(
        &mut self,
        element_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<Editor>, Subscription) {
        let editor = cx.new(|cx| Editor::single_line(window, cx));
        let subscription =
            cx.subscribe_in(&editor, window, move |this, editor, event, window, cx| {
                if !matches!(event, EditorEvent::BufferEdited) {
                    return;
                }
                let text = editor.read(cx).text(cx);
                if this.custom.input_values.get(&element_id) == Some(&text) {
                    return;
                }
                this.custom.input_values.remove(&element_id);
                this.send_event(element_id.clone(), AddonViewEvent::Change(text), window, cx);
            });
        (editor, subscription)
    }

    /// Sends an event to the addon, then renders the view again.
    pub fn send_event(
        &mut self,
        element_id: String,
        event: AddonViewEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let task = self
            .registry
            .read(cx)
            .handle_view_event(&self.key, element_id, event, cx);
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                if let Err(error) = result {
                    log::error!(
                        "addon view {:?} failed to handle an event: {error:#}",
                        this.key
                    );
                    this.error = Some(format!("{error:#}").into());
                }
                this.render_custom(window, cx);
            })
            .ok();
        })
        .detach();
    }

    fn render_element(
        &self,
        index: u32,
        depth: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(element) = self
            .custom
            .tree
            .as_ref()
            .and_then(|tree| tree.elements.get(index as usize))
        else {
            return div().into_any_element();
        };
        if depth > MAX_ELEMENT_DEPTH {
            return div().into_any_element();
        }
        let element_id =
            SharedString::from(format!("addon-element-{}-{}", self.key.view_id, element.id));
        let children = |this: &Self, window: &mut Window, cx: &mut Context<Self>| {
            element
                .children
                .iter()
                .map(|child| this.render_element(*child, depth + 1, window, cx))
                .collect::<Vec<_>>()
        };
        match &element.kind {
            AddonElementKind::Column => v_flex()
                .gap_1()
                .children(children(self, window, cx))
                .into_any_element(),
            AddonElementKind::Row => h_flex()
                .gap_1()
                .items_center()
                .children(children(self, window, cx))
                .into_any_element(),
            AddonElementKind::Scroll => v_flex()
                .id(element_id)
                .size_full()
                .gap_1()
                .overflow_y_scroll()
                .children(children(self, window, cx))
                .into_any_element(),
            AddonElementKind::List => v_flex()
                .gap_0p5()
                .children(children(self, window, cx))
                .into_any_element(),
            AddonElementKind::Label(text) => Label::new(text.clone()).into_any_element(),
            AddonElementKind::Icon(name) => {
                Icon::new(IconName::from_str(name).unwrap_or(IconName::Blocks))
                    .size(IconSize::Small)
                    .color(Color::Muted)
                    .into_any_element()
            }
            AddonElementKind::Button(label) => {
                let id = element.id.clone();
                Button::new(element_id, label.clone())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.send_event(id.clone(), AddonViewEvent::Click, window, cx);
                    }))
                    .into_any_element()
            }
            AddonElementKind::TextInput { .. } => {
                let Some((editor, _)) = self.custom.text_inputs.get(&element.id) else {
                    return div().into_any_element();
                };
                let id = element.id.clone();
                h_flex()
                    .id(element_id)
                    .px_1()
                    .py_0p5()
                    .border_1()
                    .rounded_sm()
                    .border_color(cx.theme().colors().border)
                    .on_action(cx.listener(move |this, _: &menu::Confirm, window, cx| {
                        let Some(editor) = this.text_input(&id) else {
                            return;
                        };
                        let text = editor.read(cx).text(cx);
                        this.send_event(id.clone(), AddonViewEvent::Submit(text), window, cx);
                    }))
                    .child(editor.clone())
                    .into_any_element()
            }
            AddonElementKind::Checkbox { label, checked } => {
                let id = element.id.clone();
                let view = cx.entity().downgrade();
                Checkbox::new(element_id, ToggleState::from(*checked))
                    .label(label.clone())
                    .on_click(move |state, window, cx| {
                        let checked = state.selected();
                        view.update(cx, |this, cx| {
                            this.send_event(
                                id.clone(),
                                AddonViewEvent::Toggle(checked),
                                window,
                                cx,
                            );
                        })
                        .ok();
                    })
                    .into_any_element()
            }
            AddonElementKind::Divider => Divider::horizontal().into_any_element(),
        }
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows: Vec<AnyElement> = match self.kind {
            AddonViewKind::Tree => self
                .visible_items()
                .into_iter()
                .map(|(depth, item)| self.render_tree_item(depth, item, cx))
                .collect(),
            AddonViewKind::Custom if self.custom.tree.is_some() => {
                vec![
                    div()
                        .p_2()
                        .child(self.render_element(0, 0, window, cx))
                        .into_any_element(),
                ]
            }
            AddonViewKind::Custom => Vec::new(),
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

/// An editor tab showing an addon's custom view.
pub struct AddonViewItem {
    view: Entity<AddonView>,
    title: SharedString,
}

impl AddonViewItem {
    pub fn view(&self) -> &Entity<AddonView> {
        &self.view
    }
}

pub enum AddonViewItemEvent {}

impl EventEmitter<AddonViewItemEvent> for AddonViewItem {}

impl Item for AddonViewItem {
    type Event = AddonViewItemEvent;

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        self.title.clone()
    }

    fn tab_icon(&self, _window: &Window, _cx: &App) -> Option<Icon> {
        Some(Icon::new(IconName::Blocks))
    }
}

impl Focusable for AddonViewItem {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.view.focus_handle(cx)
    }
}

impl Render for AddonViewItem {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(cx.theme().colors().editor_background)
            .child(self.view.clone())
    }
}

/// Opens a custom view in an editor tab, or activates the tab showing it.
pub(crate) fn open_view_tab(
    workspace: &mut Workspace,
    key: AddonViewKey,
    title: String,
    registry: Entity<AddonRegistry>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let existing = workspace
        .items_of_type::<AddonViewItem>(cx)
        .find(|item| item.read(cx).view.read(cx).key() == &key);
    if let Some(item) = existing {
        item.update(cx, |item, _| item.title = title.into());
        workspace.activate_item(&item, true, true, window, cx);
        return;
    }
    let view = cx.new(|cx| AddonView::new(key, AddonViewKind::Custom, registry, window, cx));
    let item = cx.new(|_| AddonViewItem {
        view,
        title: title.into(),
    });
    workspace.add_item_to_active_pane(Box::new(item), None, true, window, cx);
}
