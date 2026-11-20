mod base_keymap_setting;
mod content_into_gpui;
mod editable_setting_control;
mod editorconfig_store;
mod granted_write_path;
mod keymap_file;
mod settings_file;
mod settings_store;
mod vscode_import;

pub use settings_macros::RegisterSetting;

pub mod settings_content {
    pub use ::settings_content::*;
}

pub mod fallible_options {
    pub use ::settings_content::{FallibleOption, parse_json};
}

#[doc(hidden)]
pub mod private {
    pub use crate::settings_store::{RegisteredSetting, SettingValue};
    pub use inventory;
}

use gpui::{App, Global};

use crate as settings;
use std::env;
use std::{borrow::Cow, fmt, str};
use util::asset_str;

pub use ::settings_content::*;
pub use base_keymap_setting::*;
pub use content_into_gpui::IntoGpui;
pub use editable_setting_control::*;
pub use editorconfig_store::{
    Editorconfig, EditorconfigEvent, EditorconfigProperties, EditorconfigStore,
};
pub use granted_write_path::GrantedWritePath;
pub use keymap_file::{
    KeyBindingValidator, KeyBindingValidatorRegistration, KeybindSource, KeybindUpdateOperation,
    KeybindUpdateTarget, KeymapFile, KeymapFileLoadResult,
};
pub use settings_file::*;
pub use settings_json::*;
pub use settings_store::{
    DefaultSemanticTokenRules, InvalidSettingsError, LSP_SETTINGS_SCHEMA_URL_PREFIX,
    LocalSettingsKind, LocalSettingsPath, MigrationStatus, Settings, SettingsFile,
    SettingsJsonSchemaParams, SettingsKey, SettingsLocation, SettingsParseResult, SettingsStore,
};

pub use vscode_import::{VsCodeSettings, VsCodeSettingsSource};

pub use keymap_file::ActionSequence;

#[derive(Clone, Debug, Default, RegisterSetting)]
pub struct CopilotSettings {
    pub enterprise_uri: Option<String>,
}

impl Settings for CopilotSettings {
    fn from_settings(content: &SettingsContent) -> Self {
        Self {
            enterprise_uri: content
                .copilot
                .as_ref()
                .and_then(|copilot| copilot.enterprise_uri.clone()),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActiveSettingsProfileName(pub String);

impl Global for ActiveSettingsProfileName {}

pub trait UserSettingsContentExt {
    fn for_profile(&self, cx: &App) -> Option<&SettingsProfile>;
    fn for_release_channel(&self) -> Option<&SettingsContent>;
    fn for_os(&self) -> Option<&SettingsContent>;
}

impl UserSettingsContentExt for UserSettingsContent {
    fn for_profile(&self, cx: &App) -> Option<&SettingsProfile> {
        let Some(active_profile) = cx.try_global::<ActiveSettingsProfileName>() else {
            return None;
        };
        self.profiles.get(&active_profile.0)
    }

    fn for_release_channel(&self) -> Option<&SettingsContent> {
        self.release_channel_overrides
            .get_by_key(release_channel::RELEASE_CHANNEL.dev_name())
    }

    fn for_os(&self) -> Option<&SettingsContent> {
        self.platform_overrides.get_by_key(env::consts::OS)
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash, PartialOrd, Ord, serde::Serialize)]
pub struct WorktreeId(usize);

impl From<WorktreeId> for usize {
    fn from(value: WorktreeId) -> Self {
        value.0
    }
}

impl WorktreeId {
    pub fn from_usize(handle_id: usize) -> Self {
        Self(handle_id)
    }

    pub fn from_proto(id: u64) -> Self {
        Self(id as usize)
    }

    pub fn to_proto(self) -> u64 {
        self.0 as u64
    }

    pub fn to_usize(self) -> usize {
        self.0
    }
}

impl fmt::Display for WorktreeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

// Dev builds read the checkout's files at runtime instead of embedding them;
// see the `assets` crate for the rationale.
util::fs_embed! {
    pub struct SettingsAssets,
    crate_relative = "../../assets",
    root_relative = "assets",
    include = ["settings/*", "keymaps/*"],
    exclude = ["*.DS_Store"],
}

pub fn init(cx: &mut App) {
    let settings = SettingsStore::new(cx, &default_settings());
    cx.set_global(settings);
    SettingsStore::observe_active_settings_profile_name(cx).detach();
}

pub fn default_settings() -> Cow<'static, str> {
    asset_str::<SettingsAssets>("settings/default.json")
}

pub fn default_semantic_token_rules() -> Cow<'static, str> {
    asset_str::<SettingsAssets>("settings/default_semantic_token_rules.json")
}

#[cfg(target_os = "macos")]
pub const DEFAULT_KEYMAP_PATH: &str = "keymaps/default-macos.json";

#[cfg(target_os = "windows")]
pub const DEFAULT_KEYMAP_PATH: &str = "keymaps/default-windows.json";

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub const DEFAULT_KEYMAP_PATH: &str = "keymaps/default-linux.json";

pub fn default_keymap() -> Cow<'static, str> {
    asset_str::<SettingsAssets>(DEFAULT_KEYMAP_PATH)
}

pub const VIM_KEYMAP_PATH: &str = "keymaps/vim.json";

pub fn vim_keymap() -> Cow<'static, str> {
    asset_str::<SettingsAssets>(VIM_KEYMAP_PATH)
}

/// Specific keybinding overrides. Loaded after the base keymap so they win over
/// conflicting base-keymap (and default `Editor`) bindings for the same chords,
/// while still allowing user keymaps (loaded last) to override them. Shared
/// across features - prefer adding a context block here over creating another
/// override keymap file.
#[cfg(target_os = "macos")]
pub const SPECIFIC_OVERRIDES_KEYMAP_PATH: &str = "keymaps/specific-overrides-macos.json";

#[cfg(not(target_os = "macos"))]
pub const SPECIFIC_OVERRIDES_KEYMAP_PATH: &str = "keymaps/specific-overrides.json";

pub fn initial_user_settings_content() -> Cow<'static, str> {
    asset_str::<SettingsAssets>("settings/initial_user_settings.json")
}

pub fn initial_server_settings_content() -> Cow<'static, str> {
    asset_str::<SettingsAssets>("settings/initial_server_settings.json")
}

pub fn initial_project_settings_content() -> Cow<'static, str> {
    asset_str::<SettingsAssets>("settings/initial_local_settings.json")
}

pub fn initial_keymap_content() -> Cow<'static, str> {
    asset_str::<SettingsAssets>("keymaps/initial.json")
}

pub fn initial_tasks_content() -> Cow<'static, str> {
    asset_str::<SettingsAssets>("settings/initial_tasks.json")
}

pub fn initial_worktree_setup_tasks_content() -> Cow<'static, str> {
    asset_str::<SettingsAssets>("settings/initial_worktree_setup_tasks.json")
}

pub fn initial_debug_tasks_content() -> Cow<'static, str> {
    asset_str::<SettingsAssets>("settings/initial_debug_tasks.json")
}

pub fn initial_local_debug_tasks_content() -> Cow<'static, str> {
    asset_str::<SettingsAssets>("settings/initial_local_debug_tasks.json")
}

#[cfg(test)]
mod vireo_layout_defaults {
    use serde_json::{Value, json};

    use crate::{default_settings, parse_json_with_comments};

    fn defaults() -> Value {
        parse_json_with_comments(default_settings().as_ref())
            .expect("default settings should parse")
    }

    fn assert_default(defaults: &Value, pointer: &str, expected: Value) {
        assert_eq!(
            defaults.pointer(pointer),
            Some(&expected),
            "default for {pointer} should match Vireo's VS Code-like layout"
        );
    }

    #[test]
    fn panels_dock_like_vs_code() {
        let defaults = defaults();
        assert_default(&defaults, "/project_panel/dock", json!("left"));
        assert_default(&defaults, "/outline_panel/dock", json!("left"));
        assert_default(&defaults, "/git_panel/dock", json!("left"));
        assert_default(&defaults, "/debugger/dock", json!("left"));
        assert_default(&defaults, "/agent/dock", json!("right"));
        assert_default(&defaults, "/terminal/dock", json!("bottom"));
        assert_default(&defaults, "/project_panel/default_width", json!(300));
        assert_default(&defaults, "/outline_panel/default_width", json!(300));
        assert_default(&defaults, "/git_panel/default_width", json!(300));
    }

    #[test]
    fn keymap_and_fonts_match_vs_code_on_macos() {
        let defaults = defaults();
        assert_default(&defaults, "/base_keymap", json!("VSCode"));
        assert_default(
            &defaults,
            "/icon_theme",
            json!({ "mode": "system", "light": "Seti Light", "dark": "Seti" }),
        );
        assert_default(&defaults, "/buffer_font_family", json!("Menlo"));
        assert_default(&defaults, "/buffer_font_size", json!(12));
        assert_default(&defaults, "/buffer_line_height", json!({ "custom": 1.5 }));
        assert_default(&defaults, "/ui_font_family", json!(".SystemUIFont"));
        assert_default(&defaults, "/ui_font_size", json!(13));
    }

    #[test]
    fn editor_chrome_matches_vs_code() {
        let defaults = defaults();
        assert_default(&defaults, "/minimap/show", json!("always"));
        assert_default(&defaults, "/tabs/file_icons", json!(true));
        assert_default(&defaults, "/tabs/git_status", json!(true));
        assert_default(&defaults, "/tabs/show_diagnostics", json!("all"));
        assert_default(&defaults, "/tab_bar/show_nav_history_buttons", json!(false));
        assert_default(
            &defaults,
            "/preview_tabs/enable_preview_file_from_code_navigation",
            json!(false),
        );
        assert_default(&defaults, "/status_bar/line_endings_button", json!(true));
        assert_default(
            &defaults,
            "/status_bar/active_encoding_button",
            json!("enabled"),
        );
    }

    #[test]
    fn title_bar_matches_vs_code() {
        let defaults = defaults();
        assert_default(&defaults, "/title_bar/show_command_center", json!(true));
        assert_default(&defaults, "/title_bar/show_layout_controls", json!(true));
        // Zed-only title bar items stay available behind these settings.
        assert_default(&defaults, "/title_bar/show_collaboration", json!(false));
        assert_default(&defaults, "/title_bar/show_sign_in", json!(false));
        assert_default(&defaults, "/title_bar/show_user_menu", json!(false));
        assert_default(&defaults, "/title_bar/show_onboarding_banner", json!(false));
        // The command center names the project and the status bar shows the
        // branch, so the title bar no longer repeats them.
        assert_default(&defaults, "/title_bar/show_project_items", json!(false));
        assert_default(&defaults, "/title_bar/show_branch_name", json!(false));
    }
}
