use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::protocol::{
    AssistantMessage, CanUseToolRequest, CliMessage, ContentBlock, IncomingControlRequestBody,
    PermissionMode, RateLimitInfo, ResultMessage, StreamDelta, StreamEvent, SystemMessage, Usage,
    UserMessage,
};

#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    User(UserEntry),
    AssistantText(TextEntry),
    Thinking(ThinkingEntry),
    ToolCall(ToolCall),
    Permission(PermissionRequest),
    Notice(Notice),
}

impl Entry {
    pub fn timestamp(&self) -> DateTime<Utc> {
        match self {
            Entry::User(entry) => entry.timestamp,
            Entry::AssistantText(entry) => entry.timestamp,
            Entry::Thinking(entry) => entry.timestamp,
            Entry::ToolCall(entry) => entry.timestamp,
            Entry::Permission(entry) => entry.timestamp,
            Entry::Notice(entry) => entry.timestamp,
        }
    }

    /// Activity is what focus view hides: everything except prompts, replies,
    /// pending questions and errors.
    pub fn is_activity(&self) -> bool {
        match self {
            Entry::Thinking(_) | Entry::ToolCall(_) => true,
            Entry::Permission(request) => request.status != PermissionStatus::Pending,
            Entry::Notice(notice) => notice.kind == NoticeKind::Info,
            Entry::User(_) | Entry::AssistantText(_) => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct UserEntry {
    pub uuid: Option<String>,
    /// The id of the message just before this prompt in the session: forking
    /// "from here" resumes the session at that message.
    pub previous_uuid: Option<String>,
    pub text: String,
    pub images: usize,
    pub timestamp: DateTime<Utc>,
    pub queued: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextEntry {
    pub message_id: Option<String>,
    pub text: String,
    pub timestamp: DateTime<Utc>,
    pub streaming: bool,
    pub parent_tool_use_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ThinkingEntry {
    pub text: String,
    pub timestamp: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub parent_tool_use_id: Option<String>,
}

impl ThinkingEntry {
    pub fn duration_seconds(&self) -> Option<i64> {
        self.finished_at
            .map(|end| (end - self.timestamp).num_seconds().max(1))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolStatus {
    Streaming,
    Running,
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
    pub partial_input: String,
    pub result: Option<String>,
    pub status: ToolStatus,
    pub timestamp: DateTime<Utc>,
    pub parent_tool_use_id: Option<String>,
}

impl ToolCall {
    /// The one-line summary shown next to the tool name.
    pub fn summary(&self) -> String {
        let field = |name: &str| self.input.get(name).and_then(Value::as_str);
        let summary = match self.name.as_str() {
            "Bash" => field("description").or_else(|| field("command")),
            "Read" | "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => {
                field("file_path").or_else(|| field("notebook_path"))
            }
            "Glob" | "Grep" => field("pattern"),
            "WebFetch" => field("url"),
            "WebSearch" => field("query"),
            "Task" | "Agent" => field("description"),
            "TodoWrite" => Some("Update todos"),
            _ => field("description")
                .or_else(|| field("file_path"))
                .or_else(|| field("path")),
        };
        summary
            .unwrap_or_default()
            .lines()
            .next()
            .unwrap_or_default()
            .to_string()
    }

    pub fn file_path(&self) -> Option<&str> {
        self.input
            .get("file_path")
            .or_else(|| self.input.get("notebook_path"))
            .and_then(Value::as_str)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionStatus {
    Pending,
    Allowed,
    Denied,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PermissionRequest {
    pub request_id: String,
    pub request: CanUseToolRequest,
    pub status: PermissionStatus,
    pub timestamp: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeKind {
    Info,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub kind: NoticeKind,
    pub text: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TurnState {
    #[default]
    Idle,
    Running,
    /// Waiting for the user to answer a permission request.
    RequiresAction,
}

/// Requests from the CLI that need an answer from Vireo rather than from the
/// person: hooks (autosave) and messages for in-process MCP servers.
#[derive(Clone, Debug, PartialEq)]
pub enum HostRequest {
    HookCallback {
        request_id: String,
        callback_id: String,
        input: Value,
    },
    McpMessage {
        request_id: String,
        server_name: String,
        message: Value,
    },
    Unsupported {
        request_id: String,
    },
}

/// One conversation with the CLI, built from its output.
#[derive(Clone, Debug, Default)]
pub struct Conversation {
    pub entries: Vec<Entry>,
    pub session_id: Option<String>,
    pub title: Option<String>,
    pub model: Option<String>,
    pub permission_mode: Option<PermissionMode>,
    pub slash_commands: Vec<String>,
    pub turn: TurnState,
    pub turn_started_at: Option<DateTime<Utc>>,
    pub last_usage: Option<Usage>,
    pub total_cost_usd: f64,
    pub rate_limit: Option<RateLimitInfo>,
    pub prompt_suggestion: Option<String>,
    pub running_tool: Option<String>,
    /// Entry indices of the blocks of a streaming message, by block index.
    streaming_blocks: HashMap<usize, usize>,
    streaming_message_id: Option<String>,
    /// For each assistant message id, the entries created while it streamed,
    /// in order, and how many of them a final message has already replaced.
    streamed_entries: HashMap<String, (Vec<usize>, usize)>,
    tool_entries: HashMap<String, usize>,
    /// The id of the last main-thread message, for the next prompt's
    /// `previous_uuid`.
    last_message_uuid: Option<String>,
}

impl Conversation {
    pub fn push_user_prompt(&mut self, text: String, images: usize, now: DateTime<Utc>) {
        let queued = self.turn != TurnState::Idle;
        self.prompt_suggestion = None;
        self.entries.push(Entry::User(UserEntry {
            uuid: None,
            previous_uuid: None,
            text,
            images,
            timestamp: now,
            queued,
        }));
        if self.turn == TurnState::Idle {
            self.turn = TurnState::Running;
            self.turn_started_at = Some(now);
        }
    }

    pub fn push_notice(&mut self, kind: NoticeKind, text: impl Into<String>, now: DateTime<Utc>) {
        self.entries.push(Entry::Notice(Notice {
            kind,
            text: text.into(),
            timestamp: now,
        }));
    }

    pub fn pending_permissions(&self) -> impl Iterator<Item = &PermissionRequest> {
        self.entries.iter().filter_map(|entry| match entry {
            Entry::Permission(request) if request.status == PermissionStatus::Pending => {
                Some(request)
            }
            _ => None,
        })
    }

    pub fn set_permission_status(&mut self, request_id: &str, status: PermissionStatus) {
        for entry in self.entries.iter_mut().rev() {
            if let Entry::Permission(request) = entry
                && request.request_id == request_id
            {
                request.status = status;
                break;
            }
        }
        if self.turn == TurnState::RequiresAction && self.pending_permissions().next().is_none() {
            self.turn = TurnState::Running;
        }
    }

    pub fn tool_call(&self, tool_use_id: &str) -> Option<&ToolCall> {
        match self.entries.get(*self.tool_entries.get(tool_use_id)?) {
            Some(Entry::ToolCall(call)) => Some(call),
            _ => None,
        }
    }

    pub fn last_user_uuid(&self) -> Option<&str> {
        self.entries.iter().rev().find_map(|entry| match entry {
            Entry::User(user) => user.uuid.as_deref(),
            _ => None,
        })
    }

    /// Applies one message from the CLI. Returns a request Vireo itself must
    /// answer, if the message is one.
    pub fn apply(&mut self, message: CliMessage, now: DateTime<Utc>) -> Option<HostRequest> {
        match message {
            CliMessage::System(system) => self.apply_system(system, now),
            CliMessage::Assistant(assistant) => self.apply_assistant(assistant, now),
            CliMessage::User(user) => self.apply_user(user, now),
            CliMessage::StreamEvent(event) => {
                if event.parent_tool_use_id.is_none() {
                    self.apply_stream_event(event.event, now);
                }
            }
            CliMessage::Result(result) => self.apply_result(result, now),
            CliMessage::ControlRequest(request) => {
                return match request.request {
                    IncomingControlRequestBody::CanUseTool(can_use) => {
                        self.turn = TurnState::RequiresAction;
                        self.entries.push(Entry::Permission(PermissionRequest {
                            request_id: request.request_id,
                            request: can_use,
                            status: PermissionStatus::Pending,
                            timestamp: now,
                        }));
                        None
                    }
                    IncomingControlRequestBody::HookCallback {
                        callback_id, input, ..
                    } => Some(HostRequest::HookCallback {
                        request_id: request.request_id,
                        callback_id,
                        input,
                    }),
                    IncomingControlRequestBody::McpMessage {
                        server_name,
                        message,
                    } => Some(HostRequest::McpMessage {
                        request_id: request.request_id,
                        server_name,
                        message,
                    }),
                    IncomingControlRequestBody::Other => Some(HostRequest::Unsupported {
                        request_id: request.request_id,
                    }),
                };
            }
            CliMessage::ControlCancelRequest { request_id } => {
                self.set_permission_status(&request_id, PermissionStatus::Cancelled);
            }
            CliMessage::RateLimitEvent { rate_limit_info } => {
                self.rate_limit = Some(rate_limit_info);
            }
            CliMessage::PromptSuggestion { suggestion } => {
                let suggestion = suggestion.trim().to_string();
                self.prompt_suggestion = (!suggestion.is_empty()).then_some(suggestion);
            }
            CliMessage::ControlResponse(_) | CliMessage::Other => {}
        }
        None
    }

    fn apply_system(&mut self, system: SystemMessage, now: DateTime<Utc>) {
        match system.subtype.as_str() {
            "init" => {
                if system.session_id.is_some() {
                    self.session_id = system.session_id;
                }
                if system.model.is_some() {
                    self.model = system.model;
                }
                if let Some(mode) = system
                    .permission_mode
                    .as_deref()
                    .and_then(PermissionMode::from_cli_str)
                {
                    self.permission_mode = Some(mode);
                }
                if !system.slash_commands.is_empty() {
                    self.slash_commands = system.slash_commands;
                }
            }
            "session_state_changed" => match system.state.as_deref() {
                Some("idle") => self.finish_turn(),
                Some("running") => {
                    if self.turn == TurnState::Idle {
                        self.turn_started_at = Some(now);
                    }
                    self.turn = TurnState::Running;
                }
                Some("requires_action") => self.turn = TurnState::RequiresAction,
                _ => {}
            },
            "session_title_changed" => {
                if system.title.is_some() {
                    self.title = system.title;
                }
            }
            "compact_boundary" => self.push_notice(NoticeKind::Info, "Conversation compacted", now),
            _ => {}
        }
    }

    /// Ends the turn a restored transcript was in the middle of.
    pub fn end_restored_turn(&mut self) {
        self.finish_turn();
    }

    fn finish_turn(&mut self) {
        self.turn = TurnState::Idle;
        self.turn_started_at = None;
        self.running_tool = None;
        self.streaming_blocks.clear();
        self.streaming_message_id = None;
        for entry in &mut self.entries {
            match entry {
                Entry::AssistantText(text) => text.streaming = false,
                Entry::User(user) => user.queued = false,
                _ => {}
            }
        }
    }

    fn apply_stream_event(&mut self, event: StreamEvent, now: DateTime<Utc>) {
        match event {
            StreamEvent::MessageStart { message } => {
                self.streaming_blocks.clear();
                self.streaming_message_id = message
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
            StreamEvent::ContentBlockStart {
                index,
                content_block,
            } => {
                let entry = match content_block {
                    ContentBlock::Text { text } => Entry::AssistantText(TextEntry {
                        message_id: self.streaming_message_id.clone(),
                        text,
                        timestamp: now,
                        streaming: true,
                        parent_tool_use_id: None,
                    }),
                    ContentBlock::Thinking { thinking, .. } => Entry::Thinking(ThinkingEntry {
                        text: thinking,
                        timestamp: now,
                        finished_at: None,
                        parent_tool_use_id: None,
                    }),
                    ContentBlock::ToolUse { id, name, input } => {
                        self.running_tool = Some(name.clone());
                        let input = if input.as_object().is_some_and(|map| map.is_empty()) {
                            Value::Null
                        } else {
                            input
                        };
                        self.tool_entries.insert(id.clone(), self.entries.len());
                        Entry::ToolCall(ToolCall {
                            id,
                            name,
                            input,
                            partial_input: String::new(),
                            result: None,
                            status: ToolStatus::Streaming,
                            timestamp: now,
                            parent_tool_use_id: None,
                        })
                    }
                    _ => return,
                };
                let entry_index = self.entries.len();
                self.entries.push(entry);
                self.streaming_blocks.insert(index, entry_index);
                if let Some(message_id) = &self.streaming_message_id {
                    self.streamed_entries
                        .entry(message_id.clone())
                        .or_default()
                        .0
                        .push(entry_index);
                }
            }
            StreamEvent::ContentBlockDelta { index, delta } => {
                let Some(entry) = self
                    .streaming_blocks
                    .get(&index)
                    .and_then(|entry_index| self.entries.get_mut(*entry_index))
                else {
                    return;
                };
                match (entry, delta) {
                    (Entry::AssistantText(entry), StreamDelta::TextDelta { text }) => {
                        entry.text.push_str(&text)
                    }
                    (Entry::Thinking(entry), StreamDelta::ThinkingDelta { thinking }) => {
                        entry.text.push_str(&thinking)
                    }
                    (Entry::ToolCall(call), StreamDelta::InputJsonDelta { partial_json }) => {
                        call.partial_input.push_str(&partial_json);
                        if let Ok(input) = serde_json::from_str::<Value>(&call.partial_input) {
                            call.input = input;
                        }
                    }
                    _ => {}
                }
            }
            StreamEvent::ContentBlockStop { index } => {
                let Some(entry) = self
                    .streaming_blocks
                    .get(&index)
                    .and_then(|entry_index| self.entries.get_mut(*entry_index))
                else {
                    return;
                };
                match entry {
                    Entry::Thinking(thinking) => thinking.finished_at = Some(now),
                    Entry::AssistantText(text) => text.streaming = false,
                    Entry::ToolCall(call) => call.status = ToolStatus::Running,
                    _ => {}
                }
            }
            StreamEvent::MessageDelta { usage } => {
                if let Some(usage) = usage
                    && usage.context_tokens() > 0
                {
                    self.last_usage = Some(usage);
                }
            }
            StreamEvent::MessageStop | StreamEvent::Other => {}
        }
    }

    fn apply_assistant(&mut self, assistant: AssistantMessage, now: DateTime<Utc>) {
        if assistant.parent_tool_use_id.is_none() && assistant.uuid.is_some() {
            self.last_message_uuid = assistant.uuid.clone();
        }
        if assistant.message.model.is_some() && assistant.parent_tool_use_id.is_none() {
            self.model = assistant.message.model.clone();
        }
        if let Some(usage) = &assistant.message.usage
            && assistant.parent_tool_use_id.is_none()
            && usage.context_tokens() > 0
        {
            self.last_usage = Some(usage.clone());
        }
        let parent = assistant.parent_tool_use_id.clone();
        let message_id = assistant.message.id.clone();
        for block in assistant.message.content.blocks() {
            let streamed_index = message_id.as_ref().and_then(|id| {
                let (indices, consumed) = self.streamed_entries.get_mut(id)?;
                let index = indices.get(*consumed).copied()?;
                *consumed += 1;
                Some(index)
            });
            let existing = streamed_index.and_then(|index| self.entries.get_mut(index));
            match block {
                ContentBlock::Text { text } => match existing {
                    Some(Entry::AssistantText(entry)) => {
                        entry.text = text;
                        entry.streaming = false;
                    }
                    _ => self.entries.push(Entry::AssistantText(TextEntry {
                        message_id: message_id.clone(),
                        text,
                        timestamp: now,
                        streaming: false,
                        parent_tool_use_id: parent.clone(),
                    })),
                },
                ContentBlock::Thinking { thinking, .. } => match existing {
                    Some(Entry::Thinking(entry)) => {
                        entry.text = thinking;
                        entry.finished_at.get_or_insert(now);
                    }
                    _ => self.entries.push(Entry::Thinking(ThinkingEntry {
                        text: thinking,
                        timestamp: now,
                        finished_at: Some(now),
                        parent_tool_use_id: parent.clone(),
                    })),
                },
                ContentBlock::ToolUse { id, name, input } => {
                    if parent.is_none() {
                        self.running_tool = Some(name.clone());
                    }
                    match existing {
                        Some(Entry::ToolCall(call)) if call.id == id => {
                            call.input = input;
                            if call.status == ToolStatus::Streaming {
                                call.status = ToolStatus::Running;
                            }
                        }
                        _ => {
                            if let Some(Entry::ToolCall(call)) = self
                                .tool_entries
                                .get(&id)
                                .and_then(|index| self.entries.get_mut(*index))
                            {
                                call.input = input;
                                continue;
                            }
                            self.tool_entries.insert(id.clone(), self.entries.len());
                            self.entries.push(Entry::ToolCall(ToolCall {
                                id,
                                name,
                                input,
                                partial_input: String::new(),
                                result: None,
                                status: ToolStatus::Running,
                                timestamp: now,
                                parent_tool_use_id: parent.clone(),
                            }));
                        }
                    }
                }
                _ => {}
            }
        }
        if let Some(error) = assistant.error {
            self.push_notice(NoticeKind::Error, error, now);
        }
    }

    fn apply_user(&mut self, user: UserMessage, now: DateTime<Utc>) {
        let previous_uuid = self.last_message_uuid.clone();
        if user.parent_tool_use_id.is_none() && user.uuid.is_some() {
            self.last_message_uuid = user.uuid.clone();
        }
        let blocks = user.message.content.blocks();
        let mut has_tool_result = false;
        for block in &blocks {
            if let ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } = block
            {
                has_tool_result = true;
                if let Some(Entry::ToolCall(call)) = self
                    .tool_entries
                    .get(tool_use_id)
                    .and_then(|index| self.entries.get_mut(*index))
                {
                    call.result = Some(tool_result_text(content));
                    call.status = if is_error.unwrap_or(false) {
                        ToolStatus::Failed
                    } else {
                        ToolStatus::Succeeded
                    };
                    if call.parent_tool_use_id.is_none() {
                        self.running_tool = None;
                    }
                }
            }
        }
        if has_tool_result || user.parent_tool_use_id.is_some() {
            return;
        }

        // A replayed prompt is one Vireo sent: attach the CLI's id to it.
        let text = blocks
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        let images = blocks
            .iter()
            .filter(|block| matches!(block, ContentBlock::Image { .. }))
            .count();
        let pending = self.entries.iter_mut().find_map(|entry| match entry {
            Entry::User(entry) if entry.uuid.is_none() && entry.text == text => Some(entry),
            _ => None,
        });
        match pending {
            Some(entry) => {
                entry.uuid = user.uuid;
                entry.previous_uuid = previous_uuid;
            }
            None => {
                if text.is_empty() && images == 0 {
                    return;
                }
                self.entries.push(Entry::User(UserEntry {
                    uuid: user.uuid,
                    previous_uuid,
                    text,
                    images,
                    timestamp: now,
                    queued: false,
                }));
            }
        }
    }

    fn apply_result(&mut self, result: ResultMessage, now: DateTime<Utc>) {
        if let Some(cost) = result.total_cost_usd {
            self.total_cost_usd += cost;
        }
        if result.session_id.is_some() {
            self.session_id = result.session_id.clone();
        }
        if result.is_error {
            let text = if !result.errors.is_empty() {
                result.errors.join("\n")
            } else if let Some(text) = result.result.clone().filter(|text| !text.is_empty()) {
                text
            } else if result.subtype == "error_during_execution" {
                "Interrupted".to_string()
            } else {
                result.subtype.replace('_', " ")
            };
            self.push_notice(NoticeKind::Error, text, now);
        }
        self.finish_turn();
    }
}

pub fn tool_result_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| match item.get("type").and_then(Value::as_str) {
                Some("text") => item.get("text").and_then(Value::as_str).map(str::to_string),
                Some("image") => Some("[image]".to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply_all(conversation: &mut Conversation, lines: &[&str]) {
        for line in lines {
            conversation.apply(CliMessage::parse(line).unwrap(), Utc::now());
        }
    }

    #[test]
    fn streaming_then_final_message_does_not_duplicate() {
        let mut conversation = Conversation::default();
        conversation.push_user_prompt("list files".into(), 0, Utc::now());
        assert_eq!(conversation.turn, TurnState::Running);
        apply_all(
            &mut conversation,
            &[
                r#"{"type":"system","subtype":"init","session_id":"s1","model":"claude-opus-5-5","permissionMode":"acceptEdits","slash_commands":["compact"]}"#,
                r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"list files"}]},"uuid":"u1","isReplay":true}"#,
                r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"m1"}}}"#,
                r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}}"#,
                r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Let me"}}}"#,
                r#"{"type":"stream_event","event":{"type":"content_block_stop","index":0}}"#,
                r#"{"type":"stream_event","event":{"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}}"#,
                r#"{"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Sure"}}}"#,
                r#"{"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":", listing."}}}"#,
                r#"{"type":"assistant","message":{"id":"m1","model":"claude-opus-5-5","content":[{"type":"thinking","thinking":"Let me look","signature":"s"}]}}"#,
                r#"{"type":"assistant","message":{"id":"m1","model":"claude-opus-5-5","content":[{"type":"text","text":"Sure, listing."}]}}"#,
                r#"{"type":"stream_event","event":{"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"t1","name":"Bash","input":{}}}}"#,
                r#"{"type":"stream_event","event":{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"command\":\"ls\",\"description\":\"List files\"}"}}}"#,
                r#"{"type":"assistant","message":{"id":"m1","model":"claude-opus-5-5","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls","description":"List files"}}]}}"#,
            ],
        );
        assert_eq!(conversation.session_id.as_deref(), Some("s1"));
        assert_eq!(
            conversation.permission_mode,
            Some(PermissionMode::AcceptEdits)
        );
        assert_eq!(conversation.entries.len(), 4, "{:#?}", conversation.entries);
        let Entry::User(user) = &conversation.entries[0] else {
            panic!("expected the prompt first");
        };
        assert_eq!(user.uuid.as_deref(), Some("u1"));
        let Entry::Thinking(thinking) = &conversation.entries[1] else {
            panic!("expected thinking");
        };
        assert_eq!(thinking.text, "Let me look");
        assert!(thinking.finished_at.is_some());
        let Entry::AssistantText(text) = &conversation.entries[2] else {
            panic!("expected text");
        };
        assert_eq!(text.text, "Sure, listing.");
        let call = conversation.tool_call("t1").unwrap();
        assert_eq!(call.status, ToolStatus::Running);
        assert_eq!(call.summary(), "List files");
        assert_eq!(conversation.running_tool.as_deref(), Some("Bash"));

        apply_all(
            &mut conversation,
            &[
                r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"a.rs\nb.rs"}]}]}}"#,
                r#"{"type":"result","subtype":"success","is_error":false,"session_id":"s1","total_cost_usd":0.5}"#,
            ],
        );
        let call = conversation.tool_call("t1").unwrap();
        assert_eq!(call.status, ToolStatus::Succeeded);
        assert_eq!(call.result.as_deref(), Some("a.rs\nb.rs"));
        assert_eq!(conversation.turn, TurnState::Idle);
        assert_eq!(conversation.total_cost_usd, 0.5);
        assert_eq!(conversation.entries.len(), 4);
    }

    #[test]
    fn non_streamed_messages_and_errors() {
        let mut conversation = Conversation::default();
        apply_all(
            &mut conversation,
            &[
                r#"{"type":"assistant","message":{"id":"m2","content":[{"type":"text","text":"Hello"},{"type":"tool_use","id":"t2","name":"Read","input":{"file_path":"/a.rs"}}]}}"#,
                r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t2","content":"boom","is_error":true}]}}"#,
                r#"{"type":"result","subtype":"error_during_execution","is_error":true}"#,
            ],
        );
        assert_eq!(conversation.entries.len(), 3);
        assert_eq!(
            conversation.tool_call("t2").unwrap().status,
            ToolStatus::Failed
        );
        assert_eq!(conversation.tool_call("t2").unwrap().summary(), "/a.rs");
        let Entry::Notice(notice) = &conversation.entries[2] else {
            panic!("expected an error notice");
        };
        assert_eq!(notice.kind, NoticeKind::Error);
        assert_eq!(notice.text, "Interrupted");
    }

    #[test]
    fn permission_requests_and_host_requests() {
        let mut conversation = Conversation::default();
        conversation.push_user_prompt("edit".into(), 0, Utc::now());
        let host = conversation.apply(
            CliMessage::parse(r#"{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Edit","input":{"file_path":"/a.rs"}}}"#).unwrap(),
            Utc::now(),
        );
        assert_eq!(host, None);
        assert_eq!(conversation.turn, TurnState::RequiresAction);
        assert_eq!(conversation.pending_permissions().count(), 1);
        assert!(!conversation.entries.last().unwrap().is_activity());

        conversation.set_permission_status("r1", PermissionStatus::Allowed);
        assert_eq!(conversation.turn, TurnState::Running);
        assert_eq!(conversation.pending_permissions().count(), 0);
        assert!(conversation.entries.last().unwrap().is_activity());

        apply_all(
            &mut conversation,
            &[
                r#"{"type":"control_request","request_id":"r2","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"rm"}}}"#,
                r#"{"type":"control_cancel_request","request_id":"r2"}"#,
            ],
        );
        assert_eq!(conversation.pending_permissions().count(), 0);

        let host = conversation.apply(
            CliMessage::parse(r#"{"type":"control_request","request_id":"r3","request":{"subtype":"hook_callback","callback_id":"autosave","input":{"tool_input":{"file_path":"/a.rs"}}}}"#).unwrap(),
            Utc::now(),
        );
        assert!(
            matches!(host, Some(HostRequest::HookCallback { ref callback_id, .. }) if callback_id == "autosave")
        );
        let host = conversation.apply(
            CliMessage::parse(
                r#"{"type":"control_request","request_id":"r4","request":{"subtype":"brand_new"}}"#,
            )
            .unwrap(),
            Utc::now(),
        );
        assert_eq!(
            host,
            Some(HostRequest::Unsupported {
                request_id: "r4".into()
            })
        );
    }

    #[test]
    fn queued_prompts_suggestions_and_titles() {
        let mut conversation = Conversation::default();
        conversation.push_user_prompt("first".into(), 0, Utc::now());
        conversation.push_user_prompt("second".into(), 1, Utc::now());
        let Entry::User(second) = &conversation.entries[1] else {
            panic!("expected a prompt");
        };
        assert!(second.queued);
        assert_eq!(second.images, 1);
        apply_all(
            &mut conversation,
            &[
                r#"{"type":"system","subtype":"session_title_changed","title":"Two prompts"}"#,
                r#"{"type":"result","subtype":"success","is_error":false}"#,
                r#"{"type":"prompt_suggestion","suggestion":"  run tests "}"#,
            ],
        );
        assert_eq!(conversation.title.as_deref(), Some("Two prompts"));
        assert_eq!(conversation.prompt_suggestion.as_deref(), Some("run tests"));
        let Entry::User(second) = &conversation.entries[1] else {
            panic!("expected a prompt");
        };
        assert!(!second.queued);
        conversation.push_user_prompt("third".into(), 0, Utc::now());
        assert_eq!(conversation.prompt_suggestion, None);
    }

    #[test]
    fn subagent_stream_events_are_ignored_but_final_messages_kept() {
        let mut conversation = Conversation::default();
        apply_all(
            &mut conversation,
            &[
                r#"{"type":"stream_event","parent_tool_use_id":"t9","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":"x"}}}"#,
                r#"{"type":"assistant","parent_tool_use_id":"t9","message":{"id":"m9","content":[{"type":"tool_use","id":"t10","name":"Grep","input":{"pattern":"foo"}}]}}"#,
            ],
        );
        assert_eq!(conversation.entries.len(), 1);
        let call = conversation.tool_call("t10").unwrap();
        assert_eq!(call.parent_tool_use_id.as_deref(), Some("t9"));
        assert_eq!(conversation.running_tool, None);
    }

    #[test]
    fn prompts_remember_the_message_before_them() {
        let mut conversation = Conversation::default();
        for line in [
            r#"{"type":"user","message":{"role":"user","content":"first"},"uuid":"u1"}"#,
            r#"{"type":"assistant","uuid":"a1","message":{"id":"m1","content":[{"type":"text","text":"ok"}]}}"#,
            r#"{"type":"assistant","uuid":"sub","parent_tool_use_id":"t9","message":{"id":"m2","content":[{"type":"text","text":"subagent"}]}}"#,
        ] {
            conversation.apply(CliMessage::parse(line).unwrap(), Utc::now());
        }
        conversation.push_user_prompt("second".into(), 0, Utc::now());
        conversation.apply(
            CliMessage::parse(
                r#"{"type":"user","message":{"role":"user","content":"second"},"uuid":"u2","isReplay":true}"#,
            )
            .unwrap(),
            Utc::now(),
        );
        let users: Vec<_> = conversation
            .entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::User(user) => Some((user.uuid.clone(), user.previous_uuid.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            users,
            vec![
                (Some("u1".into()), None),
                (Some("u2".into()), Some("a1".into())),
            ]
        );
    }
}
