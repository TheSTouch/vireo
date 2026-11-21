use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context as _, Result};
use chrono::{DateTime, Utc};
use gpui::{App, AppContext as _, Entity, EntityId, Global};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::conversation::Conversation;
use crate::protocol::CliMessage;
use crate::session::ClaudeSession;
const MAX_CLOSED: usize = 20;

/// Sessions whose tab or panel view was closed, most recent last, for
/// "Reopen Closed Session".
#[derive(Default)]
struct ClosedSessions {
    session_ids: Vec<String>,
    tab_ids: Vec<EntityId>,
    last_closed_was_session: bool,
}

impl Global for ClosedSessions {}

pub fn remember_closed(session: &Entity<ClaudeSession>, cx: &mut App) {
    let session = session.read(cx);
    if session.conversation.entries.is_empty() {
        return;
    }
    let Some(session_id) = session.session_id().map(str::to_string) else {
        return;
    };
    let closed = cx.default_global::<ClosedSessions>();
    closed.session_ids.retain(|id| *id != session_id);
    closed.session_ids.push(session_id);
    if closed.session_ids.len() > MAX_CLOSED {
        closed.session_ids.remove(0);
    }
    closed.last_closed_was_session = true;
}

/// Records that a Claude tab is closing, so that the workspace's next
/// "item removed" event is known to be a Claude session.
pub fn note_claude_tab_closed(tab_id: EntityId, cx: &mut App) {
    let closed = cx.default_global::<ClosedSessions>();
    closed.tab_ids.push(tab_id);
    if closed.tab_ids.len() > MAX_CLOSED {
        closed.tab_ids.remove(0);
    }
}

/// Called for every item closed in the workspace: Cmd+Shift+T reopens a
/// Claude session only when a Claude tab was the last thing closed.
pub fn note_item_removed(item_id: EntityId, cx: &mut App) {
    let closed = cx.default_global::<ClosedSessions>();
    closed.last_closed_was_session = closed.tab_ids.contains(&item_id);
}

pub fn last_closed_was_session(cx: &App) -> bool {
    cx.try_global::<ClosedSessions>()
        .is_some_and(|closed| closed.last_closed_was_session && !closed.session_ids.is_empty())
}

pub fn take_last_closed(cx: &mut App) -> Option<String> {
    let closed = cx.default_global::<ClosedSessions>();
    let session_id = closed.session_ids.pop();
    closed.last_closed_was_session = !closed.session_ids.is_empty();
    session_id
}

/// The folder where the CLI keeps the transcripts of sessions started in
/// `cwd`: every character other than a letter or digit becomes `-`.
pub fn transcripts_dir(claude_dir: &Path, cwd: &Path) -> PathBuf {
    let encoded: String = cwd
        .to_string_lossy()
        .chars()
        .map(|char| {
            if char.is_ascii_alphanumeric() {
                char
            } else {
                '-'
            }
        })
        .collect();
    claude_dir.join("projects").join(encoded)
}

/// What Vireo remembers about a session beyond its transcript.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unread: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub archived: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

/// Vireo's session state, kept in its data folder.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionStore {
    #[serde(default)]
    pub sessions: BTreeMap<String, SessionMeta>,
    /// The sessions open in each workspace folder when Vireo last saved, and
    /// whether each was in the middle of a turn.
    #[serde(default)]
    pub open: BTreeMap<String, Vec<OpenSession>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OpenSession {
    pub session_id: String,
    #[serde(default)]
    pub interrupted: bool,
    #[serde(default)]
    pub plan_mode: bool,
    #[serde(default)]
    pub in_tab: bool,
}

impl SessionStore {
    pub fn path() -> PathBuf {
        paths::data_dir().join("claude_code").join("sessions.json")
    }

    pub fn load_from(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("failed to write {}", path.display()))
    }

    pub fn meta_mut(&mut self, session_id: &str) -> &mut SessionMeta {
        self.sessions.entry(session_id.to_string()).or_default()
    }
}

#[derive(Default)]
struct GlobalSessionStore(SessionStore);

impl Global for GlobalSessionStore {}

fn ensure_store(cx: &mut App) {
    if !cx.has_global::<GlobalSessionStore>() {
        cx.set_global(GlobalSessionStore(SessionStore::load_from(
            &SessionStore::path(),
        )));
    }
}

/// Changes the store and writes it to disk.
pub fn update_store(cx: &mut App, f: impl FnOnce(&mut SessionStore)) {
    ensure_store(cx);
    let store = cx.global_mut::<GlobalSessionStore>();
    f(&mut store.0);
    let snapshot = store.0.clone();
    if cx.is_test() {
        return;
    }
    cx.background_spawn(async move {
        if let Err(error) = snapshot.save_to(&SessionStore::path()) {
            log::warn!("failed to save Claude Code sessions: {error:#}");
        }
    })
    .detach();
}

pub fn load_store(cx: &mut App) {
    ensure_store(cx);
}

/// The title the person gave a session, which wins over the CLI's.
pub fn custom_title(session_id: &str, cx: &App) -> Option<String> {
    cx.try_global::<GlobalSessionStore>()?
        .0
        .sessions
        .get(session_id)?
        .title
        .clone()
}

pub fn read_store(cx: &mut App) -> SessionStore {
    ensure_store(cx);
    cx.global::<GlobalSessionStore>().0.clone()
}

/// One past session, as shown in the sessions list.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionSummary {
    pub session_id: String,
    pub title: String,
    pub modified: DateTime<Utc>,
    pub message_count: usize,
    pub meta: SessionMeta,
}

impl SessionSummary {
    pub fn display_title(&self) -> &str {
        self.meta.title.as_deref().unwrap_or(&self.title)
    }
}

/// Reads the title and size of a transcript without parsing every record.
pub fn summarize_transcript(path: &Path) -> Option<(String, usize)> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut ai_title = None;
    let mut custom_title = None;
    let mut summary = None;
    let mut first_prompt = None;
    let mut message_count = 0;
    for line in text.lines() {
        let is_user = line.contains("\"type\":\"user\"");
        if is_user || line.contains("\"type\":\"assistant\"") {
            message_count += 1;
        }
        let interesting = line.contains("\"type\":\"ai-title\"")
            || line.contains("\"type\":\"custom-title\"")
            || line.contains("\"type\":\"summary\"")
            || (first_prompt.is_none() && is_user);
        if !interesting {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let text_field = |key: &str| record.get(key).and_then(Value::as_str).map(str::to_string);
        match record.get("type").and_then(Value::as_str) {
            Some("ai-title") => ai_title = text_field("aiTitle"),
            Some("custom-title") => custom_title = text_field("customTitle"),
            Some("summary") => summary = text_field("summary"),
            Some("user") if first_prompt.is_none() => {
                if is_hidden_record(&record) {
                    continue;
                }
                first_prompt = prompt_text(&record).filter(|text| !is_internal_prompt(text));
            }
            _ => {}
        }
    }
    if message_count == 0 {
        return None;
    }
    let title = custom_title
        .or(ai_title)
        .or(summary)
        .or(first_prompt.map(|prompt| {
            crate::view::truncate(prompt.trim().lines().next().unwrap_or_default(), 80)
        }))
        .unwrap_or_else(|| "Untitled session".to_string());
    Some((title, message_count))
}

fn is_hidden_record(record: &Value) -> bool {
    record.get("isSidechain").and_then(Value::as_bool) == Some(true)
        || record.get("isMeta").and_then(Value::as_bool) == Some(true)
}

fn prompt_text(record: &Value) -> Option<String> {
    let content = record.get("message")?.get("content")?;
    match content {
        Value::String(text) => Some(text.clone()),
        Value::Array(blocks) => blocks.iter().find_map(|block| {
            if block.get("type").and_then(Value::as_str) == Some("text") {
                block
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            } else {
                None
            }
        }),
        _ => None,
    }
}

fn is_internal_prompt(text: &str) -> bool {
    let text = text.trim_start();
    text.starts_with("<command-")
        || text.starts_with("<local-command")
        || text.starts_with("Caveat:")
        || text.starts_with("<system-reminder>")
}

/// Lists the sessions of a workspace folder, newest first.
pub fn list_sessions(dir: &Path, store: &SessionStore) -> Vec<SessionSummary> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut sessions: Vec<SessionSummary> = entries
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "jsonl")
        })
        .filter_map(|entry| {
            let path = entry.path();
            let session_id = path.file_stem()?.to_string_lossy().into_owned();
            let modified: DateTime<Utc> = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH)
                .into();
            let (title, message_count) = summarize_transcript(&path)?;
            Some(SessionSummary {
                meta: store.sessions.get(&session_id).cloned().unwrap_or_default(),
                session_id,
                title,
                modified,
                message_count,
            })
        })
        .collect();
    sessions.sort_by_key(|session| std::cmp::Reverse(session.modified));
    sessions
}

/// The most recent session of a workspace folder that is not archived.
pub fn newest_session(dir: &Path, store: &SessionStore) -> Option<String> {
    list_sessions(dir, store)
        .into_iter()
        .find(|session| !session.meta.archived)
        .map(|session| session.session_id)
}

/// Sessions idle for longer than `days` that are not open, running or unread.
pub fn sessions_to_archive(
    sessions: &[SessionSummary],
    days: u32,
    open: &[String],
    now: DateTime<Utc>,
) -> Vec<String> {
    if days == 0 {
        return Vec::new();
    }
    let cutoff = now - chrono::Duration::days(days as i64);
    sessions
        .iter()
        .filter(|session| {
            session.modified < cutoff
                && !session.meta.archived
                && !session.meta.unread
                && !open.contains(&session.session_id)
        })
        .map(|session| session.session_id.clone())
        .collect()
}

/// Rebuilds a conversation from its transcript, for a resumed session.
pub fn load_transcript(path: &Path) -> Result<Conversation> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    Ok(conversation_from_transcript(&text))
}

pub fn conversation_from_transcript(text: &str) -> Conversation {
    let mut conversation = Conversation::default();
    let mut title = None;
    for line in text.lines() {
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let kind = record.get("type").and_then(Value::as_str);
        match kind {
            Some("ai-title") if title.is_none() => {
                title = record
                    .get("aiTitle")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                continue;
            }
            Some("custom-title") => {
                title = record
                    .get("customTitle")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                continue;
            }
            Some("user") | Some("assistant") => {}
            _ => continue,
        }
        if is_hidden_record(&record) {
            continue;
        }
        if kind == Some("user")
            && prompt_text(&record).is_some_and(|text| is_internal_prompt(&text))
        {
            continue;
        }
        let timestamp = record
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|timestamp| DateTime::parse_from_rfc3339(timestamp).ok())
            .map(|timestamp| timestamp.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);
        let session_id = record
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Ok(message) = serde_json::from_value::<CliMessage>(record) {
            conversation.apply(message, timestamp);
        }
        if session_id.is_some() {
            conversation.session_id = session_id;
        }
    }
    conversation.title = title;
    conversation.end_restored_turn();
    conversation
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRANSCRIPT: &str = concat!(
        r#"{"type":"user","sessionId":"s1","uuid":"u1","timestamp":"2026-11-01T10:00:00Z","message":{"role":"user","content":"Fix the login bug"}}"#,
        "\n",
        r#"{"type":"assistant","sessionId":"s1","uuid":"a1","timestamp":"2026-11-01T10:00:05Z","message":{"id":"m1","role":"assistant","model":"claude-opus-5-5","content":[{"type":"text","text":"Looking."},{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/p/login.rs"}}]}}"#,
        "\n",
        r#"{"type":"user","sessionId":"s1","uuid":"u2","timestamp":"2026-11-01T10:00:06Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"fn login() {}"}]}}"#,
        "\n",
        r#"{"type":"assistant","isSidechain":true,"sessionId":"s1","timestamp":"2026-11-01T10:00:07Z","message":{"id":"m9","content":[{"type":"text","text":"subagent"}]}}"#,
        "\n",
        r#"{"type":"user","isMeta":true,"sessionId":"s1","timestamp":"2026-11-01T10:00:08Z","message":{"role":"user","content":"<command-name>/clear</command-name>"}}"#,
        "\n",
        r#"{"type":"ai-title","sessionId":"s1","aiTitle":"Login bug"}"#,
        "\n",
    );

    #[test]
    fn transcript_dir_encoding() {
        assert_eq!(
            transcripts_dir(
                Path::new("/h/.claude"),
                Path::new("/Users/stan/Desktop/VS Patched")
            ),
            PathBuf::from("/h/.claude/projects/-Users-stan-Desktop-VS-Patched")
        );
        assert_eq!(
            transcripts_dir(Path::new("/h/.claude"), Path::new("/a/b.c_d")),
            PathBuf::from("/h/.claude/projects/-a-b-c-d")
        );
    }

    #[test]
    fn rebuilds_conversations_from_transcripts() {
        let conversation = conversation_from_transcript(TRANSCRIPT);
        assert_eq!(conversation.session_id.as_deref(), Some("s1"));
        assert_eq!(conversation.title.as_deref(), Some("Login bug"));
        assert_eq!(conversation.entries.len(), 3, "{:#?}", conversation.entries);
        let call = conversation.tool_call("t1").unwrap();
        assert_eq!(call.result.as_deref(), Some("fn login() {}"));
        assert_eq!(conversation.turn, crate::conversation::TurnState::Idle);
    }

    #[test]
    fn lists_and_archives_sessions() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("s1.jsonl"), TRANSCRIPT).unwrap();
        std::fs::write(
            dir.path().join("s2.jsonl"),
            r#"{"type":"user","message":{"role":"user","content":"Second session\nmore"}}"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("empty.jsonl"), "").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "x").unwrap();
        let mut store = SessionStore::default();
        store.meta_mut("s2").title = Some("Renamed".into());
        let sessions = list_sessions(dir.path(), &store);
        assert_eq!(sessions.len(), 2);
        let s1 = sessions
            .iter()
            .find(|session| session.session_id == "s1")
            .unwrap();
        assert_eq!(s1.title, "Login bug");
        let s2 = sessions
            .iter()
            .find(|session| session.session_id == "s2")
            .unwrap();
        assert_eq!(s2.title, "Second session");
        assert_eq!(s2.display_title(), "Renamed");

        let later = Utc::now() + chrono::Duration::days(30);
        let archive = sessions_to_archive(&sessions, 14, &["s2".to_string()], later);
        assert_eq!(archive, vec!["s1".to_string()]);
        assert!(sessions_to_archive(&sessions, 0, &[], later).is_empty());
        assert!(sessions_to_archive(&sessions, 14, &[], Utc::now()).is_empty());
    }

    #[gpui::test]
    fn reopen_only_after_closing_a_claude_tab(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let store = settings::SettingsStore::test(cx);
            cx.set_global(store);
            <crate::settings::ClaudeCodeSettings as settings::Settings>::register(cx);
            let tab = cx.new(|_| ());
            let other = cx.new(|_| ());
            let session = cx.new(|cx| {
                let mut session = ClaudeSession::new(
                    gpui::WeakEntity::new_invalid(),
                    PathBuf::from("/p"),
                    None,
                    cx,
                );
                session.conversation.session_id = Some("s1".into());
                session
                    .conversation
                    .push_user_prompt("hi".into(), 0, Utc::now());
                session
            });
            remember_closed(&session, cx);
            note_claude_tab_closed(tab.entity_id(), cx);
            note_item_removed(tab.entity_id(), cx);
            assert!(last_closed_was_session(cx));
            note_item_removed(other.entity_id(), cx);
            assert!(!last_closed_was_session(cx));
            note_item_removed(tab.entity_id(), cx);
            assert_eq!(take_last_closed(cx).as_deref(), Some("s1"));
            assert!(!last_closed_was_session(cx));
        });
    }

    #[test]
    fn store_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("claude_code/sessions.json");
        let mut store = SessionStore::default();
        store.meta_mut("s1").unread = true;
        store.meta_mut("s1").group = Some("Bugs".into());
        store.open.insert(
            "/p".into(),
            vec![OpenSession {
                session_id: "s1".into(),
                interrupted: true,
                plan_mode: false,
                in_tab: false,
            }],
        );
        store.save_to(&path).unwrap();
        assert_eq!(SessionStore::load_from(&path), store);
        assert_eq!(
            SessionStore::load_from(&dir.path().join("missing")),
            SessionStore::default()
        );
    }
}
