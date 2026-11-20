//! Vireo addon contributions: the manifest entries and host types behind the
//! addon UI API (WIT v0.9.0).

use std::sync::Arc;

use collections::BTreeMap;
use gpui::App;
use serde::{Deserialize, Serialize};

use crate::{Extension, ExtensionHostProxy};

/// The addon UI contributions declared in an `extension.toml`.
///
/// These keys sit at the top level of the manifest, next to Zed's own keys.
#[derive(Clone, Default, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub struct AddonManifest {
    /// Commands, keyed by command ID, that appear in the command palette and can
    /// be bound in keymaps through `addon::RunCommand`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub commands: BTreeMap<Arc<str>, AddonCommandManifestEntry>,
    /// Dock panels, keyed by panel ID. The panel ID is also the ID of the
    /// view the panel shows.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub panels: BTreeMap<Arc<str>, AddonPanelManifestEntry>,
}

impl AddonManifest {
    /// Returns whether the manifest declares no addon contributions.
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty() && self.panels.is_empty()
    }
}

/// A `[commands.<id>]` entry.
#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub struct AddonCommandManifestEntry {
    /// The title shown in the command palette.
    pub title: String,
    /// An optional longer description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A `[panels.<id>]` entry: a dock panel showing one addon view.
#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub struct AddonPanelManifestEntry {
    /// The panel title, also shown as the tooltip of its dock button.
    pub title: String,
    /// The name of a Vireo icon, such as `folder`. Defaults to a generic icon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// The dock the panel opens in. Users can move it.
    #[serde(default)]
    pub dock: AddonDock,
    /// What the panel shows.
    #[serde(default)]
    pub view: AddonViewKind,
    /// Commands offered when a tree item is right-clicked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_menu: Vec<AddonContextMenuEntry>,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AddonDock {
    #[default]
    Left,
    Right,
    Bottom,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AddonViewKind {
    /// A tree whose items the addon returns from `tree-children`.
    #[default]
    Tree,
}

/// A `[[panels.<id>.context_menu]]` entry.
#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub struct AddonContextMenuEntry {
    /// The command to run, with the item ID as its only argument.
    pub command: Arc<str>,
    /// Offer the command only on items with this `context-value`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
}

/// An item of an addon tree view.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AddonTreeItem {
    /// The item ID, unique within the view. Passed back to the addon as the
    /// parent when the item expands, and to commands run on the item.
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    pub icon: Option<String>,
    /// Whether the item has children and can expand.
    pub collapsible: bool,
    /// The command run, with the item ID as argument, when the item is clicked.
    pub command: Option<String>,
    /// Matched against the `when` of context menu entries.
    pub context_value: Option<String>,
}

/// A status bar item that an addon shows with `set-status-item`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AddonStatusItem {
    /// The item's ID, unique within the addon.
    pub id: Arc<str>,
    pub text: String,
    pub tooltip: Option<String>,
    /// The addon command run when the item is clicked.
    pub command: Option<Arc<str>>,
}

/// Connects the extension host to Vireo's addon UI (the `addon_ui` crate).
pub trait ExtensionAddonProxy: Send + Sync + 'static {
    /// Called when a Wasm extension has loaded. Every Wasm extension is
    /// registered, as an addon may use the UI API without declaring anything
    /// in its manifest.
    fn register_addon(&self, extension: Arc<dyn Extension>, cx: &mut App);

    /// Called when an extension unloads. Removes everything it contributed.
    fn unregister_addon(&self, extension_id: Arc<str>, cx: &mut App);

    /// Shows a status bar item, or updates the addon's item with the same ID.
    ///
    /// Addons may call this from their `init`, before they are registered.
    fn set_status_item(&self, extension_id: Arc<str>, item: AddonStatusItem, cx: &mut App);

    /// Removes one of the addon's status bar items.
    fn remove_status_item(&self, extension_id: Arc<str>, item_id: Arc<str>, cx: &mut App);

    /// Asks every place that shows one of the addon's views to ask the addon
    /// for its content again.
    fn refresh_view(&self, extension_id: Arc<str>, view_id: Arc<str>, cx: &mut App);
}

impl ExtensionAddonProxy for ExtensionHostProxy {
    fn register_addon(&self, extension: Arc<dyn Extension>, cx: &mut App) {
        let Some(proxy) = self.addon_proxy() else {
            return;
        };
        proxy.register_addon(extension, cx)
    }

    fn unregister_addon(&self, extension_id: Arc<str>, cx: &mut App) {
        let Some(proxy) = self.addon_proxy() else {
            return;
        };
        proxy.unregister_addon(extension_id, cx)
    }

    fn set_status_item(&self, extension_id: Arc<str>, item: AddonStatusItem, cx: &mut App) {
        let Some(proxy) = self.addon_proxy() else {
            return;
        };
        proxy.set_status_item(extension_id, item, cx)
    }

    fn remove_status_item(&self, extension_id: Arc<str>, item_id: Arc<str>, cx: &mut App) {
        let Some(proxy) = self.addon_proxy() else {
            return;
        };
        proxy.remove_status_item(extension_id, item_id, cx)
    }

    fn refresh_view(&self, extension_id: Arc<str>, view_id: Arc<str>, cx: &mut App) {
        let Some(proxy) = self.addon_proxy() else {
            return;
        };
        proxy.refresh_view(extension_id, view_id, cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ExtensionManifest;

    #[test]
    fn test_parse_addon_commands() {
        let manifest: ExtensionManifest = toml::from_str(
            r#"
            id = "hello-ui"
            name = "Hello UI"
            version = "0.1.0"
            schema_version = 1

            [commands.say-hello]
            title = "Say Hello"
            description = "Greets you."
            "#,
        )
        .unwrap();
        let command = &manifest.addon.commands["say-hello"];
        assert_eq!(command.title, "Say Hello");
        assert_eq!(command.description.as_deref(), Some("Greets you."));
        assert!(!manifest.addon.is_empty());
    }

    #[test]
    fn test_parse_addon_panels() {
        let manifest: ExtensionManifest = toml::from_str(
            r#"
            id = "hello-ui"
            name = "Hello UI"
            version = "0.1.0"
            schema_version = 1

            [panels.files]
            title = "Hello Files"
            icon = "folder"
            dock = "right"

            [[panels.files.context_menu]]
            command = "open-item"
            when = "file"

            [panels.minimal]
            title = "Minimal"
            "#,
        )
        .unwrap();
        let files = &manifest.addon.panels["files"];
        assert_eq!(files.title, "Hello Files");
        assert_eq!(files.icon.as_deref(), Some("folder"));
        assert_eq!(files.dock, AddonDock::Right);
        assert_eq!(files.view, AddonViewKind::Tree);
        assert_eq!(
            files.context_menu,
            [AddonContextMenuEntry {
                command: "open-item".into(),
                when: Some("file".into()),
            }]
        );
        let minimal = &manifest.addon.panels["minimal"];
        assert_eq!(minimal.dock, AddonDock::Left);
        assert!(minimal.context_menu.is_empty());
        assert!(!manifest.addon.is_empty());
    }
}
