//! The user interface side of Vireo addons: the commands, status bar items,
//! panels, views and editor decorations that Wasm extensions contribute
//! through extension API v0.9.0.

mod status_bar;

pub use status_bar::AddonStatusItems;

use std::sync::Arc;

use anyhow::Result;
use collections::BTreeMap;
use command_palette_hooks::{
    CommandPaletteExtraCommand, CommandPaletteFilter, GlobalCommandPaletteExtraCommands,
};
use extension::{AddonStatusItem, Extension, ExtensionAddonProxy, ExtensionHostProxy};
use gpui::{Action, App, AppContext as _, Context, Entity, Global, SharedString, Task};
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

    GlobalCommandPaletteExtraCommands::register(cx, "addon_ui", palette_commands);
    CommandPaletteFilter::update_global(cx, |filter, _| {
        filter.hide_action_types(&[std::any::TypeId::of::<RunCommand>()]);
    });

    cx.observe_new(|workspace: &mut Workspace, window, cx| {
        workspace.register_action(|_, action: &RunCommand, _, cx| {
            run_command_in_workspace(action.clone(), cx);
        });
        if let Some(window) = window
            && let Some(registry) = AddonRegistry::global(cx)
        {
            status_bar::add_to_workspace(workspace, registry, window, cx);
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
}

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
        cx.notify();
    }

    pub fn unregister(&mut self, extension_id: &str, cx: &mut Context<Self>) {
        let removed_addon = self.addons.remove(extension_id).is_some();
        let removed_items = self.status_items.remove(extension_id).is_some();
        if removed_addon || removed_items {
            cx.notify();
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
}

fn palette_commands(cx: &App) -> Vec<CommandPaletteExtraCommand> {
    let Some(registry) = AddonRegistry::global(cx) else {
        return Vec::new();
    };
    registry
        .read(cx)
        .commands()
        .into_iter()
        .map(|command| CommandPaletteExtraCommand {
            name: command.label.to_string(),
            action: Box::new(RunCommand {
                extension: command.extension_id.to_string(),
                command: command.command_id.to_string(),
                args: Vec::new(),
            }),
        })
        .collect()
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
