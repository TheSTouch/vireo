use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use chrono::{Local, Utc};
use editor::Editor;
use fuzzy::{StringMatch, StringMatchCandidate};
use gpui::{
    App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, Task, WeakEntity,
    Window, prelude::*,
};
use picker::{Picker, PickerDelegate};
use ui::{HighlightedLabel, ListItem, ListItemSpacing, Tooltip, prelude::*};
use workspace::{ModalView, Workspace};

use crate::sessions::{SessionSummary, update_store};

/// What the sessions list does with the chosen session.
pub type OnOpen = Arc<dyn Fn(&mut Workspace, String, &mut Window, &mut Context<Workspace>)>;

/// "Past Conversations": the sessions of this workspace folder, searchable,
/// grouped, with archived sessions last.
pub struct SessionsDelegate {
    workspace: WeakEntity<Workspace>,
    sessions: Vec<SessionSummary>,
    matches: Vec<StringMatch>,
    selected_index: usize,
    show_archived: bool,
    on_open: OnOpen,
}

impl SessionsDelegate {
    pub fn new(
        workspace: WeakEntity<Workspace>,
        sessions: Vec<SessionSummary>,
        on_open: OnOpen,
    ) -> Self {
        Self {
            workspace,
            sessions,
            matches: Vec::new(),
            selected_index: 0,
            show_archived: false,
            on_open,
        }
    }

    fn visible(&self) -> Vec<(usize, String)> {
        let mut visible: Vec<(usize, &SessionSummary)> = self
            .sessions
            .iter()
            .enumerate()
            .filter(|(_, session)| self.show_archived || !session.meta.archived)
            .collect();
        // Grouped sessions first, by group name, then the rest by date.
        visible.sort_by(|(_, a), (_, b)| {
            (
                a.meta.archived,
                a.meta.group.is_none(),
                a.meta.group.clone(),
            )
                .cmp(&(
                    b.meta.archived,
                    b.meta.group.is_none(),
                    b.meta.group.clone(),
                ))
                .then(b.modified.cmp(&a.modified))
        });
        visible
            .into_iter()
            .map(|(index, session)| {
                let mut label = session.display_title().to_string();
                if let Some(group) = &session.meta.group {
                    label = format!("{group} / {label}");
                }
                (index, label)
            })
            .collect()
    }

    fn session_at(&self, match_index: usize) -> Option<&SessionSummary> {
        let found = self.matches.get(match_index)?;
        self.sessions.get(found.candidate_id)
    }
}

impl PickerDelegate for SessionsDelegate {
    type ListItem = ListItem;

    fn name() -> &'static str {
        "Claude Code sessions"
    }

    fn match_count(&self) -> usize {
        self.matches.len()
    }

    fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn set_selected_index(&mut self, index: usize, _: &mut Window, cx: &mut Context<Picker<Self>>) {
        self.selected_index = index.min(self.matches.len().saturating_sub(1));
        cx.notify();
    }

    fn placeholder_text(&self, _: &mut Window, _: &mut App) -> Arc<str> {
        "Search sessions…".into()
    }

    fn no_matches_text(&self, _: &mut Window, _: &mut App) -> Option<SharedString> {
        Some("No past conversations in this folder".into())
    }

    fn update_matches(
        &mut self,
        query: String,
        _: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Task<()> {
        self.show_archived = query.starts_with("archived:");
        let query = query.trim_start_matches("archived:").trim().to_string();
        let visible = self.visible();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |picker, cx| {
            let matches = if query.is_empty() {
                visible
                    .into_iter()
                    .map(|(index, label)| StringMatch {
                        candidate_id: index,
                        string: label,
                        positions: Vec::new(),
                        score: 0.0,
                    })
                    .collect()
            } else {
                let candidates: Vec<StringMatchCandidate> = visible
                    .iter()
                    .map(|(index, label)| StringMatchCandidate::new(*index, label))
                    .collect();
                fuzzy::match_strings(
                    &candidates,
                    &query,
                    false,
                    true,
                    200,
                    &AtomicBool::new(false),
                    executor,
                )
                .await
            };
            picker
                .update(cx, |picker, cx| {
                    picker.delegate.matches = matches;
                    picker.delegate.selected_index = 0;
                    cx.notify();
                })
                .ok();
        })
    }

    fn confirm(&mut self, _: bool, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        let Some(session) = self.session_at(self.selected_index) else {
            return;
        };
        let session_id = session.session_id.clone();
        update_store(cx, |store| store.meta_mut(&session_id).unread = false);
        let on_open = self.on_open.clone();
        self.workspace
            .update(cx, |workspace, cx| {
                on_open(workspace, session_id, window, cx)
            })
            .ok();
        cx.emit(DismissEvent);
    }

    fn dismissed(&mut self, _: &mut Window, cx: &mut Context<Picker<Self>>) {
        cx.emit(DismissEvent);
    }

    fn render_match(
        &self,
        index: usize,
        selected: bool,
        _: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let found = self.matches.get(index)?;
        let session = self.sessions.get(found.candidate_id)?;
        let session_id = session.session_id.clone();
        let archived = session.meta.archived;
        let modified = session.modified.with_timezone(&Local);
        let age = Utc::now() - session.modified;
        let when = if age.num_days() >= 1 {
            modified.format("%-d %b").to_string()
        } else {
            modified.format("%-I:%M %p").to_string()
        };
        Some(
            ListItem::new(index)
                .inset(true)
                .spacing(ListItemSpacing::Sparse)
                .toggle_state(selected)
                .start_slot(
                    Icon::new(if session.meta.unread {
                        IconName::Circle
                    } else {
                        IconName::Sparkle
                    })
                    .size(IconSize::Small)
                    .color(if session.meta.unread {
                        Color::Accent
                    } else {
                        Color::Muted
                    }),
                )
                .child(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .justify_between()
                        .child(
                            HighlightedLabel::new(found.string.clone(), found.positions.clone())
                                .truncate(),
                        )
                        .child(
                            Label::new(format!("{when} · {} messages", session.message_count))
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        ),
                )
                .end_slot(
                    IconButton::new(
                        ("archive", index),
                        if archived {
                            IconName::Undo
                        } else {
                            IconName::Close
                        },
                    )
                    .icon_size(IconSize::XSmall)
                    .tooltip(Tooltip::text(if archived {
                        "Unarchive session"
                    } else {
                        "Archive session"
                    }))
                    .on_click(cx.listener(move |picker, _, window, cx| {
                        let session_id = session_id.clone();
                        update_store(cx, |store| store.meta_mut(&session_id).archived = !archived);
                        if let Some(session) = picker
                            .delegate
                            .sessions
                            .iter_mut()
                            .find(|session| session.session_id == session_id)
                        {
                            session.meta.archived = !archived;
                        }
                        picker.refresh(window, cx);
                    })),
                ),
        )
    }
}

/// A one-line prompt, for renaming a session or naming its group.
pub struct TextPrompt {
    title: SharedString,
    editor: Entity<Editor>,
    on_submit: Box<dyn Fn(String, &mut App)>,
}

impl TextPrompt {
    pub fn new(
        title: impl Into<SharedString>,
        initial: &str,
        on_submit: impl Fn(String, &mut App) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let editor = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_text(initial, window, cx);
            editor.select_all(&editor::actions::SelectAll, window, cx);
            editor
        });
        Self {
            title: title.into(),
            editor,
            on_submit: Box::new(on_submit),
        }
    }

    fn confirm(&mut self, _: &menu::Confirm, _: &mut Window, cx: &mut Context<Self>) {
        let text = self.editor.read(cx).text(cx).trim().to_string();
        (self.on_submit)(text, cx);
        cx.emit(DismissEvent);
    }

    fn cancel(&mut self, _: &menu::Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }
}

impl EventEmitter<DismissEvent> for TextPrompt {}
impl ModalView for TextPrompt {}

impl Focusable for TextPrompt {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.editor.focus_handle(cx)
    }
}

impl Render for TextPrompt {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .key_context("ClaudeCodeTextPrompt")
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .w(rems(28.))
            .p_2()
            .gap_1()
            .elevation_3(cx)
            .child(
                Label::new(self.title.clone())
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .child(
                div()
                    .px_1()
                    .rounded_sm()
                    .border_1()
                    .border_color(cx.theme().colors().border)
                    .child(self.editor.clone()),
            )
    }
}
