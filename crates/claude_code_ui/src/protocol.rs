//! The NDJSON protocol the `claude` CLI speaks with `--input-format stream-json
//! --output-format stream-json`. Field names follow the CLI; unknown message
//! types and fields are tolerated, since the CLI adds them often.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionMode {
    Default,
    AcceptEdits,
    Plan,
    BypassPermissions,
    DontAsk,
    Auto,
}

impl PermissionMode {
    pub fn as_cli_str(self) -> &'static str {
        match self {
            PermissionMode::Default => "default",
            PermissionMode::AcceptEdits => "acceptEdits",
            PermissionMode::Plan => "plan",
            PermissionMode::BypassPermissions => "bypassPermissions",
            PermissionMode::DontAsk => "dontAsk",
            PermissionMode::Auto => "auto",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PermissionMode::Default => "Manual",
            PermissionMode::AcceptEdits => "Edit automatically",
            PermissionMode::Plan => "Plan",
            PermissionMode::BypassPermissions => "Bypass permissions",
            PermissionMode::DontAsk => "Don't ask",
            PermissionMode::Auto => "Auto",
        }
    }

    pub fn from_cli_str(value: &str) -> Option<Self> {
        Some(match value {
            "default" | "manual" => PermissionMode::Default,
            "acceptEdits" => PermissionMode::AcceptEdits,
            "plan" => PermissionMode::Plan,
            "bypassPermissions" => PermissionMode::BypassPermissions,
            "dontAsk" => PermissionMode::DontAsk,
            "auto" => PermissionMode::Auto,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Thinking {
        thinking: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    RedactedThinking {
        #[serde(default)]
        data: String,
    },
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        #[serde(default)]
        content: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
    Image {
        source: ImageSource,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImageSource {
    #[serde(rename = "type")]
    pub kind: String,
    pub media_type: String,
    pub data: String,
}

impl ImageSource {
    pub fn base64(media_type: impl Into<String>, data: impl Into<String>) -> Self {
        Self {
            kind: "base64".into(),
            media_type: media_type.into(),
            data: data.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

impl MessageContent {
    pub fn blocks(&self) -> Vec<ContentBlock> {
        match self {
            MessageContent::Text(text) => vec![ContentBlock::Text { text: text.clone() }],
            MessageContent::Blocks(blocks) => blocks.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApiMessage {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    pub content: MessageContent,
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub usage: Option<Usage>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
}

impl Usage {
    pub fn context_tokens(&self) -> u64 {
        self.input_tokens + self.cache_creation_input_tokens + self.cache_read_input_tokens
    }
}

/// A line the CLI writes on stdout.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CliMessage {
    System(SystemMessage),
    Assistant(AssistantMessage),
    User(UserMessage),
    StreamEvent(StreamEventMessage),
    Result(ResultMessage),
    ControlRequest(IncomingControlRequest),
    ControlResponse(IncomingControlResponse),
    ControlCancelRequest {
        request_id: String,
    },
    RateLimitEvent {
        #[serde(default)]
        rate_limit_info: RateLimitInfo,
    },
    PromptSuggestion {
        #[serde(default)]
        suggestion: String,
    },
    #[serde(other)]
    Other,
}

impl CliMessage {
    pub fn parse(line: &str) -> serde_json::Result<Self> {
        serde_json::from_str(line)
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct SystemMessage {
    pub subtype: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default, rename = "permissionMode")]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub slash_commands: Vec<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct AssistantMessage {
    pub message: ApiMessage,
    #[serde(default)]
    pub parent_tool_use_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub uuid: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct UserMessage {
    pub message: ApiMessage,
    #[serde(default)]
    pub parent_tool_use_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub uuid: Option<String>,
    #[serde(default, rename = "isReplay")]
    pub is_replay: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct StreamEventMessage {
    pub event: StreamEvent,
    #[serde(default)]
    pub parent_tool_use_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    MessageStart {
        message: Value,
    },
    ContentBlockStart {
        index: usize,
        content_block: ContentBlock,
    },
    ContentBlockDelta {
        index: usize,
        delta: StreamDelta,
    },
    ContentBlockStop {
        index: usize,
    },
    MessageDelta {
        #[serde(default)]
        usage: Option<Usage>,
    },
    MessageStop,
    #[serde(other)]
    Other,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamDelta {
    TextDelta {
        text: String,
    },
    ThinkingDelta {
        thinking: String,
    },
    InputJsonDelta {
        partial_json: String,
    },
    SignatureDelta {
        signature: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ResultMessage {
    pub subtype: String,
    #[serde(default)]
    pub is_error: bool,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub num_turns: u64,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub total_cost_usd: Option<f64>,
    #[serde(default)]
    pub usage: Option<Usage>,
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub errors: Vec<String>,
    /// Usage by model, which also gives each model's context window.
    #[serde(default, rename = "modelUsage")]
    pub model_usage: std::collections::BTreeMap<String, ModelUsage>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsage {
    #[serde(default)]
    pub context_window: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitInfo {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub resets_at: Option<i64>,
    #[serde(default)]
    pub rate_limit_type: Option<String>,
    #[serde(default)]
    pub utilization: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct IncomingControlRequest {
    pub request_id: String,
    pub request: IncomingControlRequestBody,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
pub enum IncomingControlRequestBody {
    CanUseTool(CanUseToolRequest),
    HookCallback {
        callback_id: String,
        #[serde(default)]
        input: Value,
        #[serde(default)]
        tool_use_id: Option<String>,
    },
    McpMessage {
        server_name: String,
        message: Value,
    },
    #[serde(other)]
    Other,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct CanUseToolRequest {
    pub tool_name: String,
    #[serde(default)]
    pub input: Value,
    #[serde(default)]
    pub permission_suggestions: Vec<Value>,
    #[serde(default)]
    pub tool_use_id: Option<String>,
    #[serde(default)]
    pub blocked_path: Option<String>,
    #[serde(default)]
    pub decision_reason: Option<Value>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct IncomingControlResponse {
    pub response: IncomingControlResponseBody,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct IncomingControlResponseBody {
    pub subtype: String,
    pub request_id: String,
    #[serde(default)]
    pub response: Option<Value>,
    #[serde(default)]
    pub error: Option<String>,
}

/// The reply to `initialize`: what the CLI can do for this session.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct InitializeResponse {
    #[serde(default)]
    pub commands: Vec<SlashCommand>,
    #[serde(default)]
    pub models: Vec<ModelInfo>,
    #[serde(default)]
    pub account: Option<AccountInfo>,
    #[serde(default)]
    pub current_permission_mode: Option<String>,
    #[serde(default)]
    pub claude_code_version: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlashCommand {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub argument_hint: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub value: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub resolved_model: Option<String>,
    #[serde(default)]
    pub supported_effort_levels: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountInfo {
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub organization: Option<String>,
    #[serde(default)]
    pub subscription_type: Option<String>,
    #[serde(default)]
    pub api_provider: Option<String>,
}

/// A line written to the CLI's stdin.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StdinMessage {
    User {
        session_id: String,
        message: OutgoingUserContent,
        parent_tool_use_id: Option<String>,
    },
    ControlRequest {
        request_id: String,
        request: OutgoingControlRequest,
    },
    ControlResponse {
        response: OutgoingControlResponse,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OutgoingUserContent {
    pub role: &'static str,
    pub content: Vec<ContentBlock>,
}

impl StdinMessage {
    pub fn user(session_id: Option<&str>, content: Vec<ContentBlock>) -> Self {
        StdinMessage::User {
            session_id: session_id.unwrap_or_default().to_string(),
            message: OutgoingUserContent {
                role: "user",
                content,
            },
            parent_tool_use_id: None,
        }
    }

    pub fn to_line(&self) -> serde_json::Result<String> {
        let mut line = serde_json::to_string(self)?;
        line.push('\n');
        Ok(line)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
pub enum OutgoingControlRequest {
    Initialize {
        hooks: Option<Value>,
        #[serde(rename = "promptSuggestions")]
        prompt_suggestions: bool,
    },
    Interrupt,
    SetPermissionMode {
        mode: PermissionMode,
    },
    SetModel {
        model: Option<String>,
    },
    SetMaxThinkingTokens {
        max_thinking_tokens: Option<u32>,
    },
    RewindFiles {
        user_message_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
pub enum OutgoingControlResponse {
    Success { request_id: String, response: Value },
    Error { request_id: String, error: String },
}

/// The answer to a `can_use_tool` request.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "behavior", rename_all = "snake_case")]
pub enum PermissionDecision {
    Allow {
        #[serde(rename = "updatedInput")]
        updated_input: Value,
        #[serde(rename = "updatedPermissions", skip_serializing_if = "Vec::is_empty")]
        updated_permissions: Vec<Value>,
    },
    Deny {
        message: String,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        interrupt: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_init_and_assistant_messages() {
        let init = CliMessage::parse(
            r#"{"type":"system","subtype":"init","session_id":"s1","model":"claude-opus-5-5","permissionMode":"default","slash_commands":["compact","review"],"tools":["Bash","Edit"],"cwd":"/p"}"#,
        )
        .unwrap();
        let CliMessage::System(system) = init else {
            panic!("expected system message");
        };
        assert_eq!(system.subtype, "init");
        assert_eq!(system.session_id.as_deref(), Some("s1"));
        assert_eq!(system.slash_commands, vec!["compact", "review"]);
        assert_eq!(system.permission_mode.as_deref(), Some("default"));

        let assistant = CliMessage::parse(
            r#"{"type":"assistant","message":{"id":"m1","role":"assistant","model":"claude-opus-5-5","content":[{"type":"thinking","thinking":"hm","signature":"x"},{"type":"text","text":"Hi"},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]},"session_id":"s1","parent_tool_use_id":null}"#,
        )
        .unwrap();
        let CliMessage::Assistant(assistant) = assistant else {
            panic!("expected assistant message");
        };
        let blocks = assistant.message.content.blocks();
        assert_eq!(blocks.len(), 3);
        assert_eq!(
            blocks[2],
            ContentBlock::ToolUse {
                id: "t1".into(),
                name: "Bash".into(),
                input: json!({"command": "ls"})
            }
        );
    }

    #[test]
    fn tolerates_unknown_types_and_blocks() {
        assert_eq!(
            CliMessage::parse(r#"{"type":"something_new","x":1}"#).unwrap(),
            CliMessage::Other
        );
        let user = CliMessage::parse(
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"},{"type":"brand_new_block"}]}}"#,
        )
        .unwrap();
        let CliMessage::User(user) = user else {
            panic!("expected user message");
        };
        assert_eq!(user.message.content.blocks()[1], ContentBlock::Unknown);

        let text_user =
            CliMessage::parse(r#"{"type":"user","message":{"role":"user","content":"hello"}}"#)
                .unwrap();
        let CliMessage::User(text_user) = text_user else {
            panic!("expected user message");
        };
        assert_eq!(
            text_user.message.content.blocks(),
            vec![ContentBlock::Text {
                text: "hello".into()
            }]
        );
    }

    #[test]
    fn parses_stream_events_and_results() {
        let delta = CliMessage::parse(
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Hel"}},"session_id":"s1"}"#,
        )
        .unwrap();
        assert_eq!(
            delta,
            CliMessage::StreamEvent(StreamEventMessage {
                event: StreamEvent::ContentBlockDelta {
                    index: 1,
                    delta: StreamDelta::TextDelta { text: "Hel".into() }
                },
                parent_tool_use_id: None,
                session_id: Some("s1".into()),
            })
        );
        let result = CliMessage::parse(
            r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":1200,"num_turns":2,"session_id":"s1","total_cost_usd":0.02,"usage":{"input_tokens":10,"output_tokens":5},"result":"done"}"#,
        )
        .unwrap();
        let CliMessage::Result(result) = result else {
            panic!("expected result");
        };
        assert_eq!(result.num_turns, 2);
        assert_eq!(result.total_cost_usd, Some(0.02));
    }

    #[test]
    fn parses_control_requests() {
        let request = CliMessage::parse(
            r#"{"type":"control_request","request_id":"r9","request":{"subtype":"can_use_tool","tool_name":"Edit","input":{"file_path":"/a.rs","old_string":"a","new_string":"b"},"permission_suggestions":[{"type":"setMode","mode":"acceptEdits","destination":"session"}],"tool_use_id":"t2"}}"#,
        )
        .unwrap();
        let CliMessage::ControlRequest(request) = request else {
            panic!("expected control request");
        };
        assert_eq!(request.request_id, "r9");
        let IncomingControlRequestBody::CanUseTool(can_use) = request.request else {
            panic!("expected can_use_tool");
        };
        assert_eq!(can_use.tool_name, "Edit");
        assert_eq!(can_use.permission_suggestions.len(), 1);

        let hook = CliMessage::parse(
            r#"{"type":"control_request","request_id":"r10","request":{"subtype":"hook_callback","callback_id":"autosave","input":{"tool_name":"Read","tool_input":{"file_path":"/a.rs"}}}}"#,
        )
        .unwrap();
        assert!(matches!(
            hook,
            CliMessage::ControlRequest(IncomingControlRequest {
                request: IncomingControlRequestBody::HookCallback { .. },
                ..
            })
        ));
    }

    #[test]
    fn parses_initialize_response() {
        let response = CliMessage::parse(
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"req_1","response":{"commands":[{"name":"review","description":"Review","argumentHint":"","builtin":true}],"models":[{"value":"opus","resolvedModel":"claude-opus-5-5","displayName":"Opus 5.5","description":"x","supportedEffortLevels":["low","high"]}],"account":{"email":"a@b.c","subscriptionType":"max"},"current_permission_mode":"default","pid":1}}}"#,
        )
        .unwrap();
        let CliMessage::ControlResponse(response) = response else {
            panic!("expected control response");
        };
        let initialize: InitializeResponse =
            serde_json::from_value(response.response.response.unwrap()).unwrap();
        assert_eq!(initialize.commands[0].name, "review");
        assert_eq!(initialize.models[0].display_name, "Opus 5.5");
        assert_eq!(
            initialize.account.unwrap().subscription_type.as_deref(),
            Some("max")
        );
    }

    #[test]
    fn serializes_stdin_messages() {
        let user = StdinMessage::user(
            None,
            vec![
                ContentBlock::Text { text: "hi".into() },
                ContentBlock::Image {
                    source: ImageSource::base64("image/png", "AAAA"),
                },
            ],
        );
        assert_eq!(
            serde_json::to_value(&user).unwrap(),
            json!({
                "type": "user",
                "session_id": "",
                "message": {"role": "user", "content": [
                    {"type": "text", "text": "hi"},
                    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AAAA"}}
                ]},
                "parent_tool_use_id": null
            })
        );
        assert!(user.to_line().unwrap().ends_with('\n'));

        let interrupt = StdinMessage::ControlRequest {
            request_id: "req_2".into(),
            request: OutgoingControlRequest::Interrupt,
        };
        assert_eq!(
            serde_json::to_value(&interrupt).unwrap(),
            json!({"type": "control_request", "request_id": "req_2", "request": {"subtype": "interrupt"}})
        );
        let mode = OutgoingControlRequest::SetPermissionMode {
            mode: PermissionMode::AcceptEdits,
        };
        assert_eq!(
            serde_json::to_value(&mode).unwrap(),
            json!({"subtype": "set_permission_mode", "mode": "acceptEdits"})
        );
    }

    #[test]
    fn serializes_permission_decisions() {
        let allow = StdinMessage::ControlResponse {
            response: OutgoingControlResponse::Success {
                request_id: "r9".into(),
                response: serde_json::to_value(PermissionDecision::Allow {
                    updated_input: json!({"command": "ls"}),
                    updated_permissions: Vec::new(),
                })
                .unwrap(),
            },
        };
        assert_eq!(
            serde_json::to_value(&allow).unwrap(),
            json!({"type": "control_response", "response": {"subtype": "success", "request_id": "r9", "response": {"behavior": "allow", "updatedInput": {"command": "ls"}}}})
        );
        let deny = PermissionDecision::Deny {
            message: "no".into(),
            interrupt: false,
        };
        assert_eq!(
            serde_json::to_value(&deny).unwrap(),
            json!({"behavior": "deny", "message": "no"})
        );
    }

    #[test]
    fn parses_suggestions_and_titles() {
        assert_eq!(
            CliMessage::parse(r#"{"type":"prompt_suggestion","suggestion":"run the tests"}"#)
                .unwrap(),
            CliMessage::PromptSuggestion {
                suggestion: "run the tests".into()
            }
        );
        let CliMessage::System(title) = CliMessage::parse(
            r#"{"type":"system","subtype":"session_title_changed","session_id":"s1","title":"Fix login"}"#,
        )
        .unwrap() else {
            panic!("expected system message");
        };
        assert_eq!(title.title.as_deref(), Some("Fix login"));
        let initialize = OutgoingControlRequest::Initialize {
            hooks: None,
            prompt_suggestions: true,
        };
        assert_eq!(
            serde_json::to_value(&initialize).unwrap(),
            json!({"subtype": "initialize", "hooks": null, "promptSuggestions": true})
        );
    }

    #[test]
    fn permission_mode_aliases() {
        assert_eq!(
            PermissionMode::from_cli_str("manual"),
            Some(PermissionMode::Default)
        );
        assert_eq!(PermissionMode::AcceptEdits.as_cli_str(), "acceptEdits");
        assert_eq!(PermissionMode::from_cli_str("nope"), None);
    }
}
