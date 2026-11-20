use gpui::{App, Entity, Global};

use crate::session::ClaudeSession;

const MAX_CLOSED: usize = 20;

/// Sessions whose tab or panel view was closed, most recent last, for
/// "Reopen Closed Session".
#[derive(Default)]
struct ClosedSessions(Vec<String>);

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
    closed.0.retain(|id| *id != session_id);
    closed.0.push(session_id);
    if closed.0.len() > MAX_CLOSED {
        closed.0.remove(0);
    }
}

pub fn take_last_closed(cx: &mut App) -> Option<String> {
    cx.default_global::<ClosedSessions>().0.pop()
}

pub fn has_closed(cx: &App) -> bool {
    cx.try_global::<ClosedSessions>()
        .is_some_and(|closed| !closed.0.is_empty())
}
