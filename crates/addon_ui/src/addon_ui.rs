//! The user interface side of Vireo addons: the commands, status bar items,
//! panels, views and editor decorations that Wasm extensions contribute
//! through extension API v0.9.0.

mod decorations;
mod panel;
mod prompt;
mod services;
mod status_bar;
mod view;

pub use decorations::{CollectionKey, Decorations};
pub use panel::{AddonPanel, PANEL_SLOTS, TogglePanel};
pub use services::AddonSettings;
pub use status_bar::AddonStatusItems;
pub use view::{AddonView, AddonViewItem};

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use collections::BTreeMap;
use command_palette_hooks::{
    CommandPaletteExtraCommand, CommandPaletteFilter, GlobalCommandPaletteExtraCommands,
};
use extension::{
    AddonActiveEditor, AddonContextMenuEntry, AddonDecoration, AddonNotificationLevel,
    AddonPanelManifestEntry, AddonPosition, AddonQuickPickItem, AddonStatusItem, AddonTreeItem,
    AddonViewEvent, AddonViewTree, Extension, ExtensionAddonProxy, ExtensionHostProxy,
};
use futures::future::BoxFuture;
use gpui::{
    Action, App, AppContext as _, BorrowAppContext as _, Context, Entity, EventEmitter, Global,
    SharedString, Task,
};
use schemars::JsonSchema;
use serde::Deserialize;
use workspace::{Toast, Workspace, notifications::NotificationId};

/// Runs a command contributed by an addon.
///
/// Bind it in a keymap with
/// `["addon::RunCommand", { "extension": "<extension id>", "command": "<command id>" }]`.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = addon)]
#[serde(deny_unknown_fields)]
pub struct RunCommand {
    /// The ID of the extension that declares the command.
    pub extension: String,
    /// The command ID, as declared under `[commands.<id>]`.
    pub command: String,
    /// Arguments passed to the addon.
    #[serde(default)]
    pub args: Vec<String>,
}

pub fn init(proxy: Arc<ExtensionHostProxy>, cx: &mut App) {
    let registry = cx.new(|_| AddonRegistry::default());
    cx.set_global(GlobalAddonRegistry(registry));
    proxy.register_addon_proxy(AddonProxy);
    decorations::init(cx);

    GlobalCommandPaletteExtraCommands::register(cx, "addon_ui", palette_commands);
    CommandPaletteFilter::update_global(cx, |filter, _| {
        filter.hide_action_types(&[
            std::any::TypeId::of::<RunCommand>(),
            std::any::TypeId::of::<TogglePanel>(),
        ]);
    });

    cx.observe_new(|workspace: &mut Workspace, window, cx| {
        workspace.register_action(|_, action: &RunCommand, _, cx| {
            run_command_in_workspace(action.clone(), cx);
        });
        workspace.register_action(|workspace, action: &TogglePanel, window, cx| {
            if let Some(registry) = AddonRegistry::global(cx) {
                panel::toggle_panel(workspace, action, &registry, window, cx);
            }
        });
        if let Some(window) = window
            && let Some(registry) = AddonRegistry::global(cx)
        {
            status_bar::add_to_workspace(workspace, registry.clone(), window, cx);
            panel::sync_panels(workspace, &registry, window, cx);
            cx.observe_in(&registry, window, |workspace, registry, window, cx| {
                panel::sync_panels(workspace, &registry, window, cx);
            })
            .detach();
        }
    })
    .detach();
}

struct GlobalAddonRegistry(Entity<AddonRegistry>);

impl Global for GlobalAddonRegistry {}

/// Every loaded Wasm extension, with what it contributes to the UI.
#[derive(Default)]
pub struct AddonRegistry {
    addons: BTreeMap<Arc<str>, Arc<dyn Extension>>,
    /// Status bar items by addon, in the order the addon first set them.
    status_items: BTreeMap<Arc<str>, Vec<AddonStatusItem>>,
    /// The addon panel shown by each `AddonPanel<SLOT>` type.
    panel_slots: [Option<AddonViewKey>; PANEL_SLOTS],
    /// File watches by watch ID, with the addon that started them.
    watches: BTreeMap<u64, (Arc<str>, Task<()>)>,
    next_watch_id: u64,
}

/// Identifies a view: an addon and the view ID it chose. A panel's view ID is
/// the panel ID.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AddonViewKey {
    pub extension_id: Arc<str>,
    pub view_id: Arc<str>,
}

impl AddonViewKey {
    pub fn new(extension_id: impl Into<Arc<str>>, view_id: impl Into<Arc<str>>) -> Self {
        Self {
            extension_id: extension_id.into(),
            view_id: view_id.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AddonRegistryEvent {
    /// The addon asked for a view to show fresh content.
    RefreshView(AddonViewKey),
}

impl EventEmitter<AddonRegistryEvent> for AddonRegistry {}

/// An addon command, as listed in the command palette.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddonCommand {
    pub extension_id: Arc<str>,
    pub command_id: Arc<str>,
    /// "<extension name>: <command title>".
    pub label: SharedString,
}

impl AddonRegistry {
    pub fn global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalAddonRegistry>()
            .map(|global| global.0.clone())
    }

    pub fn addon(&self, extension_id: &str) -> Option<Arc<dyn Extension>> {
        self.addons.get(extension_id).cloned()
    }

    pub fn commands(&self) -> Vec<AddonCommand> {
        self.addons
            .iter()
            .flat_map(|(extension_id, extension)| {
                let manifest = extension.manifest();
                manifest
                    .addon
                    .commands
                    .iter()
                    .map(|(command_id, command)| AddonCommand {
                        extension_id: extension_id.clone(),
                        command_id: command_id.clone(),
                        label: format!("{}: {}", manifest.name, command.title).into(),
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    pub fn register(&mut self, extension: Arc<dyn Extension>, cx: &mut Context<Self>) {
        let extension_id = extension.manifest().id.clone();
        self.addons.insert(extension_id, extension);
        self.assign_panel_slots();
        cx.notify();
    }

    pub fn unregister(&mut self, extension_id: &str, cx: &mut Context<Self>) {
        let removed_addon = self.addons.remove(extension_id).is_some();
        let removed_items = self.status_items.remove(extension_id).is_some();
        self.watches
            .retain(|_, (watch_extension_id, _)| watch_extension_id.as_ref() != extension_id);
        if removed_addon || removed_items {
            self.assign_panel_slots();
            cx.notify();
        }
    }

    /// Frees the slots of panels that are gone and gives free slots to new
    /// panels, keeping existing panels in their slots.
    fn assign_panel_slots(&mut self) {
        let stale_slots: Vec<usize> = (0..PANEL_SLOTS)
            .filter(|&slot| {
                self.panel_slots[slot]
                    .as_ref()
                    .is_some_and(|key| self.panel(key).is_none())
            })
            .collect();
        for slot in stale_slots {
            self.panel_slots[slot] = None;
        }
        for (extension_id, extension) in &self.addons {
            for panel_id in extension.manifest().addon.panels.keys() {
                let key = AddonViewKey::new(extension_id.clone(), panel_id.clone());
                if self.panel_slots.contains(&Some(key.clone())) {
                    continue;
                }
                match self.panel_slots.iter_mut().find(|slot| slot.is_none()) {
                    Some(slot) => *slot = Some(key),
                    None => log::warn!(
                        "not showing addon panel {}/{}: only {PANEL_SLOTS} addon panels can be shown",
                        key.extension_id,
                        key.view_id
                    ),
                }
            }
        }
    }

    /// The manifest entry of a panel of a registered addon.
    pub fn panel(&self, key: &AddonViewKey) -> Option<AddonPanelManifestEntry> {
        self.addons
            .get(&key.extension_id)?
            .manifest()
            .addon
            .panels
            .get(&key.view_id)
            .cloned()
    }

    pub fn panel_in_slot(&self, slot: usize) -> Option<AddonViewKey> {
        self.panel_slots.get(slot).cloned().flatten()
    }

    pub fn slot_of_panel(&self, key: &AddonViewKey) -> Option<usize> {
        self.panel_slots
            .iter()
            .position(|slot| slot.as_ref() == Some(key))
    }

    /// Every panel of every addon, with the title shown in the command palette.
    pub fn panels(&self) -> Vec<(AddonViewKey, SharedString)> {
        self.addons
            .iter()
            .flat_map(|(extension_id, extension)| {
                let manifest = extension.manifest();
                manifest
                    .addon
                    .panels
                    .iter()
                    .map(|(panel_id, panel)| {
                        (
                            AddonViewKey::new(extension_id.clone(), panel_id.clone()),
                            format!("{}: Toggle {}", manifest.name, panel.title).into(),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Asks every place that shows the view to ask the addon for its content again.
    pub fn refresh_view(&mut self, key: AddonViewKey, cx: &mut Context<Self>) {
        cx.emit(AddonRegistryEvent::RefreshView(key));
    }

    /// The context menu commands offered on a tree item, with their titles.
    pub fn context_menu_entries(
        &self,
        key: &AddonViewKey,
        context_value: Option<&str>,
    ) -> Vec<(Arc<str>, SharedString)> {
        let Some(extension) = self.addons.get(&key.extension_id) else {
            return Vec::new();
        };
        let manifest = extension.manifest();
        let Some(panel) = manifest.addon.panels.get(&key.view_id) else {
            return Vec::new();
        };
        panel
            .context_menu
            .iter()
            .filter(|entry: &&AddonContextMenuEntry| {
                entry
                    .when
                    .as_deref()
                    .is_none_or(|when| Some(when) == context_value)
            })
            .filter_map(|entry| {
                let command = manifest.addon.commands.get(&entry.command)?;
                Some((entry.command.clone(), command.title.clone().into()))
            })
            .collect()
    }

    /// Asks an addon for the content of a custom view.
    pub fn render_view(&self, key: &AddonViewKey, cx: &App) -> Task<Result<AddonViewTree>> {
        let Some(extension) = self.addon(&key.extension_id) else {
            return Task::ready(Err(anyhow::anyhow!(
                "no addon with ID `{}` is loaded",
                key.extension_id
            )));
        };
        let view_id = key.view_id.clone();
        cx.background_spawn(async move { extension.addon_render_view(view_id).await })
    }

    /// Sends an event from an element of a custom view to its addon.
    pub fn handle_view_event(
        &self,
        key: &AddonViewKey,
        element_id: String,
        event: AddonViewEvent,
        cx: &App,
    ) -> Task<Result<()>> {
        let Some(extension) = self.addon(&key.extension_id) else {
            return Task::ready(Err(anyhow::anyhow!(
                "no addon with ID `{}` is loaded",
                key.extension_id
            )));
        };
        let view_id = key.view_id.clone();
        cx.background_spawn(async move {
            extension
                .addon_handle_view_event(view_id, element_id, event)
                .await
        })
    }

    /// Asks an addon for the children of a tree view item.
    pub fn tree_children(
        &self,
        key: &AddonViewKey,
        parent: Option<Arc<str>>,
        cx: &App,
    ) -> Task<Result<Vec<AddonTreeItem>>> {
        let Some(extension) = self.addon(&key.extension_id) else {
            return Task::ready(Err(anyhow::anyhow!(
                "no addon with ID `{}` is loaded",
                key.extension_id
            )));
        };
        let view_id = key.view_id.clone();
        cx.background_spawn(async move {
            extension
                .addon_tree_children(view_id, parent.map(|parent| parent.to_string()))
                .await
        })
    }

    pub(crate) fn next_watch_id(&mut self) -> u64 {
        self.next_watch_id += 1;
        self.next_watch_id
    }

    pub(crate) fn add_watch(&mut self, extension_id: Arc<str>, watch_id: u64, task: Task<()>) {
        self.watches.insert(watch_id, (extension_id, task));
    }

    /// Stops a watch, if the addon started it.
    pub(crate) fn remove_watch(&mut self, extension_id: &str, watch_id: u64) {
        if self
            .watches
            .get(&watch_id)
            .is_some_and(|(watch_extension_id, _)| watch_extension_id.as_ref() == extension_id)
        {
            self.watches.remove(&watch_id);
        }
    }

    /// Every addon's status bar items, ordered by addon ID.
    pub fn status_items(&self) -> Vec<(Arc<str>, AddonStatusItem)> {
        self.status_items
            .iter()
            .flat_map(|(extension_id, items)| {
                items
                    .iter()
                    .map(|item| (extension_id.clone(), item.clone()))
            })
            .collect()
    }

    /// Shows a status bar item, or updates the addon's item with the same ID.
    ///
    /// Accepted for addons that are not registered yet, as addons set their
    /// items while they initialize.
    pub fn set_status_item(
        &mut self,
        extension_id: Arc<str>,
        item: AddonStatusItem,
        cx: &mut Context<Self>,
    ) {
        let items = self.status_items.entry(extension_id).or_default();
        match items.iter_mut().find(|existing| existing.id == item.id) {
            Some(existing) if *existing == item => return,
            Some(existing) => *existing = item,
            None => items.push(item),
        }
        cx.notify();
    }

    pub fn remove_status_item(
        &mut self,
        extension_id: &str,
        item_id: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(items) = self.status_items.get_mut(extension_id) else {
            return;
        };
        let count = items.len();
        items.retain(|item| item.id.as_ref() != item_id);
        if items.len() != count {
            if items.is_empty() {
                self.status_items.remove(extension_id);
            }
            cx.notify();
        }
    }

    /// Runs an addon command. Fails when the addon is not loaded or does not
    /// declare the command.
    pub fn run_command(
        &self,
        extension_id: &str,
        command_id: &str,
        args: Vec<String>,
        cx: &App,
    ) -> Task<Result<()>> {
        let Some(extension) = self.addon(extension_id) else {
            return Task::ready(Err(anyhow::anyhow!(
                "no addon with ID `{extension_id}` is loaded"
            )));
        };
        let Some((command_id, _)) = extension
            .manifest()
            .addon
            .commands
            .get_key_value(command_id)
            .map(|(id, entry)| (id.clone(), entry.clone()))
        else {
            return Task::ready(Err(anyhow::anyhow!(
                "addon `{extension_id}` declares no command `{command_id}`"
            )));
        };
        cx.background_spawn(async move { extension.run_addon_command(command_id, args).await })
    }
}

struct AddonProxy;

impl ExtensionAddonProxy for AddonProxy {
    fn register_addon(&self, extension: Arc<dyn Extension>, cx: &mut App) {
        if let Some(registry) = AddonRegistry::global(cx) {
            registry.update(cx, |registry, cx| registry.register(extension, cx));
        }
    }

    fn unregister_addon(&self, extension_id: Arc<str>, cx: &mut App) {
        if let Some(registry) = AddonRegistry::global(cx) {
            registry.update(cx, |registry, cx| registry.unregister(&extension_id, cx));
        }
        if cx.has_global::<Decorations>() {
            let paths = cx.update_global(|decorations: &mut Decorations, _| {
                decorations.clear_extension(&extension_id)
            });
            decorations::refresh(Some(&paths), cx);
        }
    }

    fn set_status_item(&self, extension_id: Arc<str>, item: AddonStatusItem, cx: &mut App) {
        if let Some(registry) = AddonRegistry::global(cx) {
            registry.update(cx, |registry, cx| {
                registry.set_status_item(extension_id, item, cx)
            });
        }
    }

    fn remove_status_item(&self, extension_id: Arc<str>, item_id: Arc<str>, cx: &mut App) {
        if let Some(registry) = AddonRegistry::global(cx) {
            registry.update(cx, |registry, cx| {
                registry.remove_status_item(&extension_id, &item_id, cx)
            });
        }
    }

    fn refresh_view(&self, extension_id: Arc<str>, view_id: Arc<str>, cx: &mut App) {
        if let Some(registry) = AddonRegistry::global(cx) {
            registry.update(cx, |registry, cx| {
                registry.refresh_view(AddonViewKey::new(extension_id, view_id), cx)
            });
        }
    }

    fn set_decorations(
        &self,
        extension_id: Arc<str>,
        path: PathBuf,
        collection: Arc<str>,
        decorations: Vec<AddonDecoration>,
        cx: &mut App,
    ) {
        if !cx.has_global::<Decorations>() {
            return;
        }
        let key = CollectionKey {
            extension_id,
            collection,
        };
        cx.update_global(|all: &mut Decorations, _| all.set(key, path.clone(), decorations));
        decorations::refresh(Some(&[path].into_iter().collect()), cx);
    }

    fn clear_decorations(&self, extension_id: Arc<str>, collection: Arc<str>, cx: &mut App) {
        if !cx.has_global::<Decorations>() {
            return;
        }
        let key = CollectionKey {
            extension_id,
            collection,
        };
        let paths = cx.update_global(|all: &mut Decorations, _| all.clear(&key));
        decorations::refresh(Some(&paths), cx);
    }

    fn active_editor(&self, cx: &mut App) -> Option<AddonActiveEditor> {
        services::active_editor(cx)
    }

    fn active_editor_text(&self, cx: &mut App) -> Option<String> {
        services::active_editor_text(cx)
    }

    fn open_file(
        &self,
        path: PathBuf,
        position: Option<AddonPosition>,
        cx: &mut App,
    ) -> BoxFuture<'static, Result<()>> {
        services::open_file(path, position, cx)
    }

    fn show_notification(
        &self,
        extension_id: Arc<str>,
        level: AddonNotificationLevel,
        message: String,
        cx: &mut App,
    ) {
        services::show_notification(extension_id, level, message, cx)
    }

    fn quick_pick(
        &self,
        items: Vec<AddonQuickPickItem>,
        placeholder: Option<String>,
        cx: &mut App,
    ) -> BoxFuture<'static, Option<String>> {
        services::quick_pick(items, placeholder, cx)
    }

    fn input_box(
        &self,
        prompt: String,
        value: Option<String>,
        cx: &mut App,
    ) -> BoxFuture<'static, Option<String>> {
        services::input_box(prompt, value, cx)
    }

    fn workspace_folders(&self, cx: &mut App) -> Vec<PathBuf> {
        services::workspace_folders(cx)
    }

    fn watch_path(&self, extension_id: Arc<str>, path: PathBuf, cx: &mut App) -> u64 {
        services::watch_path(extension_id, path, cx)
    }

    fn unwatch_path(&self, extension_id: Arc<str>, watch_id: u64, cx: &mut App) {
        services::unwatch_path(extension_id, watch_id, cx)
    }

    fn addon_settings(&self, extension_id: Arc<str>, cx: &mut App) -> Option<String> {
        services::addon_settings(&extension_id, cx)
    }

    fn storage_get(&self, extension_id: Arc<str>, key: String, cx: &mut App) -> Option<String> {
        services::storage_get(&extension_id, &key, cx)
    }

    fn storage_set(
        &self,
        extension_id: Arc<str>,
        key: String,
        value: Option<String>,
        cx: &mut App,
    ) -> BoxFuture<'static, Result<()>> {
        services::storage_set(&extension_id, key, value, cx)
    }

    fn open_view_tab(
        &self,
        extension_id: Arc<str>,
        view_id: Arc<str>,
        title: String,
        cx: &mut App,
    ) {
        workspace::with_active_or_new_workspace(cx, move |workspace, window, cx| {
            if let Some(registry) = AddonRegistry::global(cx) {
                let key = AddonViewKey::new(extension_id, view_id);
                view::open_view_tab(workspace, key, title, registry, window, cx);
            }
        });
    }
}

fn palette_commands(cx: &App) -> Vec<CommandPaletteExtraCommand> {
    let Some(registry) = AddonRegistry::global(cx) else {
        return Vec::new();
    };
    let registry = registry.read(cx);
    let commands = registry
        .commands()
        .into_iter()
        .map(|command| CommandPaletteExtraCommand {
            name: command.label.to_string(),
            action: Box::new(RunCommand {
                extension: command.extension_id.to_string(),
                command: command.command_id.to_string(),
                args: Vec::new(),
            }),
        });
    let panels = registry
        .panels()
        .into_iter()
        .map(|(key, label)| CommandPaletteExtraCommand {
            name: label.to_string(),
            action: Box::new(TogglePanel {
                extension: key.extension_id.to_string(),
                panel: key.view_id.to_string(),
            }),
        });
    commands.chain(panels).collect()
}

struct AddonCommandError;

/// Runs an addon command and reports a failure as a workspace notification.
fn run_command_in_workspace(action: RunCommand, cx: &mut Context<Workspace>) {
    let Some(registry) = AddonRegistry::global(cx) else {
        return;
    };
    let task = registry
        .read(cx)
        .run_command(&action.extension, &action.command, action.args, cx);
    cx.spawn(async move |workspace, cx| {
        if let Err(error) = task.await {
            log::error!("addon command failed: {error:#}");
            workspace
                .update(cx, |workspace, cx| {
                    workspace.show_toast(
                        Toast::new(
                            NotificationId::composite::<AddonCommandError>(SharedString::from(
                                format!("{}::{}", action.extension, action.command),
                            )),
                            format!("Command `{}` failed: {error:#}", action.command),
                        ),
                        cx,
                    )
                })
                .ok();
        }
    })
    .detach();
}

#[cfg(test)]
mod addon_ui_tests;
