use settings::{
    ClaudeCodeOpensIn, ClaudeCodePermissionMode, ClaudeCodePreferredLocation, ClaudeCodeSpinnerVerbs,
    ClaudeCodeSpinnerVerbsMode, RegisterSetting, Settings, SettingsContent,
};

use crate::protocol::PermissionMode;

#[derive(Clone, Debug, PartialEq, RegisterSetting)]
pub struct ClaudeCodeSettings {
    pub environment_variables: Vec<(String, String)>,
    pub use_terminal: bool,
    pub allow_dangerously_skip_permissions: bool,
    pub claude_process_wrapper: Option<String>,
    pub respect_git_ignore: bool,
    pub initial_permission_mode: Option<PermissionMode>,
    pub disable_login_prompt: bool,
    pub autosave: bool,
    pub focus_view: bool,
    pub use_ctrl_enter_to_send: bool,
    pub preferred_location: ClaudeCodePreferredLocation,
    pub new_conversation_opens_in: ClaudeCodeOpensIn,
    pub past_conversation_opens_in: ClaudeCodeOpensIn,
    pub lock_editor_groups: bool,
    pub enable_new_conversation_shortcut: bool,
    pub enable_reopen_closed_session_shortcut: bool,
    pub hide_onboarding: bool,
    pub attach_open_file: bool,
    pub attach_selection: bool,
    pub continue_after_reload: bool,
    pub scroll_to_bottom_on_send: bool,
    pub show_message_timestamps: bool,
    pub prompt_suggestions: bool,
    pub spinner_verbs: ClaudeCodeSpinnerVerbs,
    pub archive_inactive_sessions: u32,
    pub use_python_environment: bool,
}

impl Settings for ClaudeCodeSettings {
    fn from_settings(content: &SettingsContent) -> Self {
        let claude = content.claude_code.clone().unwrap_or_default();
        Self {
            environment_variables: claude
                .environment_variables
                .unwrap_or_default()
                .into_iter()
                .map(|variable| (variable.name, variable.value))
                .collect(),
            use_terminal: claude.use_terminal.unwrap_or(false),
            allow_dangerously_skip_permissions: claude
                .allow_dangerously_skip_permissions
                .unwrap_or(false),
            claude_process_wrapper: claude
                .claude_process_wrapper
                .filter(|wrapper| !wrapper.trim().is_empty()),
            respect_git_ignore: claude.respect_git_ignore.unwrap_or(true),
            initial_permission_mode: claude.initial_permission_mode.map(|mode| match mode {
                ClaudeCodePermissionMode::Default | ClaudeCodePermissionMode::Manual => {
                    PermissionMode::Default
                }
                ClaudeCodePermissionMode::AcceptEdits => PermissionMode::AcceptEdits,
                ClaudeCodePermissionMode::Plan => PermissionMode::Plan,
                ClaudeCodePermissionMode::BypassPermissions => PermissionMode::BypassPermissions,
                ClaudeCodePermissionMode::Auto => PermissionMode::Auto,
            }),
            disable_login_prompt: claude.disable_login_prompt.unwrap_or(false),
            autosave: claude.autosave.unwrap_or(true),
            focus_view: claude.focus_view.unwrap_or(false),
            use_ctrl_enter_to_send: claude.use_ctrl_enter_to_send.unwrap_or(false),
            preferred_location: claude.preferred_location.unwrap_or_default(),
            new_conversation_opens_in: claude.new_conversation_opens_in.unwrap_or_default(),
            past_conversation_opens_in: claude.past_conversation_opens_in.unwrap_or_default(),
            lock_editor_groups: claude.lock_editor_groups.unwrap_or(true),
            enable_new_conversation_shortcut: claude
                .enable_new_conversation_shortcut
                .unwrap_or(false),
            enable_reopen_closed_session_shortcut: claude
                .enable_reopen_closed_session_shortcut
                .unwrap_or(true),
            hide_onboarding: claude.hide_onboarding.unwrap_or(false),
            attach_open_file: claude.attach_open_file.unwrap_or(true),
            attach_selection: claude.attach_selection.unwrap_or(true),
            continue_after_reload: claude.continue_after_reload.unwrap_or(true),
            scroll_to_bottom_on_send: claude.scroll_to_bottom_on_send.unwrap_or(true),
            show_message_timestamps: claude.show_message_timestamps.unwrap_or(true),
            prompt_suggestions: claude.prompt_suggestions.unwrap_or(true),
            spinner_verbs: claude.spinner_verbs.unwrap_or_default(),
            archive_inactive_sessions: claude.archive_inactive_sessions.unwrap_or(14),
            use_python_environment: claude.use_python_environment.unwrap_or(true),
        }
    }
}

/// The verbs the spinner cycles through while Claude works.
pub const BUILT_IN_SPINNER_VERBS: &[&str] = &[
    "Working",
    "Thinking",
    "Pondering",
    "Considering",
    "Reasoning",
    "Exploring",
    "Crafting",
    "Composing",
    "Tinkering",
    "Computing",
];

pub fn spinner_verbs(settings: &ClaudeCodeSpinnerVerbs) -> Vec<String> {
    let custom = settings
        .verbs
        .iter()
        .map(|verb| verb.trim())
        .filter(|verb| !verb.is_empty())
        .map(str::to_string);
    match settings.mode {
        ClaudeCodeSpinnerVerbsMode::Replace if !settings.verbs.is_empty() => custom.collect(),
        _ => BUILT_IN_SPINNER_VERBS
            .iter()
            .map(|verb| verb.to_string())
            .chain(custom)
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn defaults_match_the_extension(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let store = settings::SettingsStore::test(cx);
            cx.set_global(store);
            ClaudeCodeSettings::register(cx);
            let settings = ClaudeCodeSettings::get_global(cx);
            assert!(settings.autosave);
            assert!(settings.attach_open_file);
            assert!(settings.attach_selection);
            assert!(settings.continue_after_reload);
            assert!(settings.enable_reopen_closed_session_shortcut);
            assert!(!settings.enable_new_conversation_shortcut);
            assert!(!settings.use_ctrl_enter_to_send);
            assert!(!settings.allow_dangerously_skip_permissions);
            assert_eq!(settings.archive_inactive_sessions, 14);
            assert_eq!(
                settings.preferred_location,
                ClaudeCodePreferredLocation::Panel
            );
            assert_eq!(
                settings.new_conversation_opens_in,
                ClaudeCodeOpensIn::NewTab
            );
            assert_eq!(
                settings.past_conversation_opens_in,
                ClaudeCodeOpensIn::NewTab
            );
            assert_eq!(settings.initial_permission_mode, None);
            assert_eq!(settings.claude_process_wrapper, None);
            assert!(settings.environment_variables.is_empty());
        });
    }

    #[test]
    fn spinner_verbs_append_and_replace() {
        let append = ClaudeCodeSpinnerVerbs {
            mode: ClaudeCodeSpinnerVerbsMode::Append,
            verbs: vec!["Brewing".into(), " ".into()],
        };
        let verbs = spinner_verbs(&append);
        assert_eq!(verbs.len(), BUILT_IN_SPINNER_VERBS.len() + 1);
        assert_eq!(verbs.last().map(String::as_str), Some("Brewing"));

        let replace = ClaudeCodeSpinnerVerbs {
            mode: ClaudeCodeSpinnerVerbsMode::Replace,
            verbs: vec!["Brewing".into()],
        };
        assert_eq!(spinner_verbs(&replace), vec!["Brewing".to_string()]);

        let empty_replace = ClaudeCodeSpinnerVerbs {
            mode: ClaudeCodeSpinnerVerbsMode::Replace,
            verbs: Vec::new(),
        };
        assert_eq!(
            spinner_verbs(&empty_replace).len(),
            BUILT_IN_SPINNER_VERBS.len()
        );
    }
}
