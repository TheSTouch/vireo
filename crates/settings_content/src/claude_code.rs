use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use settings_macros::{MergeFrom, with_fallible_options};

/// Settings of Vireo's Claude Code panel.
#[with_fallible_options]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema, MergeFrom)]
pub struct ClaudeCodeSettingsContent {
    /// Environment variables to set when launching Claude. Prefer setting them in
    /// Claude's own settings.json.
    ///
    /// Default: []
    pub environment_variables: Option<Vec<ClaudeCodeEnvironmentVariable>>,
    /// Launch Claude in the terminal instead of the native panel.
    ///
    /// Default: false
    pub use_terminal: Option<bool>,
    /// Allow the bypass permissions mode. Recommended only for sandboxes with no
    /// internet access.
    ///
    /// Default: false
    pub allow_dangerously_skip_permissions: Option<bool>,
    /// Executable used to launch the Claude process. It receives the path of
    /// `claude` and its arguments.
    ///
    /// Default: null
    pub claude_process_wrapper: Option<String>,
    /// Respect .gitignore files when searching files for @-mentions.
    ///
    /// Default: true
    pub respect_git_ignore: Option<bool>,
    /// Initial permission mode for new conversations. Unset defers to the Claude
    /// Code CLI's own default. `manual` is an alias for `default`.
    ///
    /// Default: null
    pub initial_permission_mode: Option<ClaudeCodePermissionMode>,
    /// Never prompt for login in the panel, when authentication is handled
    /// outside Vireo.
    ///
    /// Default: false
    pub disable_login_prompt: Option<bool>,
    /// Save files automatically before Claude reads or writes them.
    ///
    /// Default: true
    pub autosave: Option<bool>,
    /// Focus view: hide tool calls and in-progress activity, showing only your
    /// prompts and Claude's responses.
    ///
    /// Default: false
    pub focus_view: Option<bool>,
    /// Use Ctrl/Cmd+Enter to send prompts, so that Enter inserts a newline.
    ///
    /// Default: false
    pub use_ctrl_enter_to_send: Option<bool>,
    /// Where Claude opens by default. Updated when you open Claude elsewhere.
    ///
    /// Default: panel
    pub preferred_location: Option<ClaudeCodePreferredLocation>,
    /// Keep files opened from a Claude tab out of the pane that holds it.
    ///
    /// Default: true
    pub lock_editor_groups: Option<bool>,
    /// Use Cmd/Ctrl+N to start a new conversation while Claude is focused.
    ///
    /// Default: false
    pub enable_new_conversation_shortcut: Option<bool>,
    /// Use Cmd/Ctrl+Shift+T to reopen the most recently closed Claude session.
    ///
    /// Default: true
    pub enable_reopen_closed_session_shortcut: Option<bool>,
    /// Hide the onboarding checklist.
    ///
    /// Default: false
    pub hide_onboarding: Option<bool>,
    /// Add the file open in the editor to your messages.
    ///
    /// Default: true
    pub attach_open_file: Option<bool>,
    /// Add the text selected in the editor to your messages.
    ///
    /// Default: true
    pub attach_selection: Option<bool>,
    /// After Vireo restarts, a restored session continues the step that was
    /// interrupted.
    ///
    /// Default: true
    pub continue_after_reload: Option<bool>,
    /// Scroll the conversation to the bottom when you send a message.
    ///
    /// Default: true
    pub scroll_to_bottom_on_send: Option<bool>,
    /// Show when each message was sent, with a date line where the day changes.
    ///
    /// Default: true
    pub show_message_timestamps: Option<bool>,
    /// Show a suggested next prompt in the message box after Claude replies.
    ///
    /// Default: true
    pub prompt_suggestions: Option<bool>,
    /// Verbs shown while Claude works.
    ///
    /// Default: {"mode": "append", "verbs": []}
    pub spinner_verbs: Option<ClaudeCodeSpinnerVerbs>,
    /// Archive a session after this many days with no activity. 0 never
    /// archives. Open, running, waiting or unread sessions are never archived.
    ///
    /// Default: 14
    pub archive_inactive_sessions: Option<u32>,
    /// Activate the workspace's Python virtual environment when running Claude.
    ///
    /// Default: true
    pub use_python_environment: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, MergeFrom)]
pub struct ClaudeCodeEnvironmentVariable {
    pub name: String,
    pub value: String,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, MergeFrom,
)]
#[serde(rename_all = "camelCase")]
pub enum ClaudeCodePermissionMode {
    Default,
    Manual,
    AcceptEdits,
    Plan,
    BypassPermissions,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, MergeFrom,
)]
#[serde(rename_all = "snake_case")]
pub enum ClaudeCodePreferredLocation {
    Sidebar,
    #[default]
    Panel,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, MergeFrom)]
pub struct ClaudeCodeSpinnerVerbs {
    #[serde(default)]
    pub mode: ClaudeCodeSpinnerVerbsMode,
    #[serde(default)]
    pub verbs: Vec<String>,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, MergeFrom,
)]
#[serde(rename_all = "snake_case")]
pub enum ClaudeCodeSpinnerVerbsMode {
    #[default]
    Append,
    Replace,
}
