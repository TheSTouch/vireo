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
}

impl AddonManifest {
    /// Returns whether the manifest declares no addon contributions.
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
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

/// Connects the extension host to Vireo's addon UI (the `addon_ui` crate).
pub trait ExtensionAddonProxy: Send + Sync + 'static {
    /// Called when a Wasm extension has loaded. Every Wasm extension is
    /// registered, as an addon may use the UI API without declaring anything
    /// in its manifest.
    fn register_addon(&self, extension: Arc<dyn Extension>, cx: &mut App);

    /// Called when an extension unloads. Removes everything it contributed.
    fn unregister_addon(&self, extension_id: Arc<str>, cx: &mut App);
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
}

#[cfg(test)]
mod tests {
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
}
