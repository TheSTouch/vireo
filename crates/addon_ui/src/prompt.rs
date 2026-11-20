//! Quick picks and input boxes that addons open and wait on.

use std::sync::Arc;

use extension::AddonQuickPickItem;
use futures::channel::oneshot;
use fuzzy::{StringMatch, StringMatchCandidate, match_strings};
use gpui::{App, DismissEvent, Entity, EventEmitter, Focusable, Task, WeakEntity, prelude::*};
use picker::{Picker, PickerDelegate};
use ui::{HighlightedLabel, ListItem, ListItemSpacing, prelude::*};
use workspace::{ModalView, Workspace};

/// A modal that answers an addon's `quick-pick` or `input-box` call.
pub struct AddonPrompt {
    picker: Entity<Picker<AddonPromptDelegate>>,
}

impl ModalView for AddonPrompt {}

impl EventEmitter<DismissEvent> for AddonPrompt {}

impl Focusable for AddonPrompt {
    fn focus_handle(&self, cx: &App) -> gpui::FocusHandle {
        self.picker.focus_handle(cx)
    }
}

impl Render for AddonPrompt {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        v_flex().w(rems(34.)).child(self.picker.clone())
    }
}

enum PromptKind {
    /// Pick one of the items; answers with the item ID.
    QuickPick(Vec<AddonQuickPickItem>),
    /// Type a line of text; answers with the text.
    InputBox,
}

pub struct AddonPromptDelegate {
    prompt: WeakEntity<AddonPrompt>,
    kind: PromptKind,
    placeholder: Arc<str>,
    matches: Vec<StringMatch>,
    selected_index: usize,
    query: String,
    answer: Option<oneshot::Sender<Option<String>>>,
}

impl AddonPromptDelegate {
    fn answer(&mut self, value: Option<String>) {
        if let Some(answer) = self.answer.take() {
            answer.send(value).ok();
        }
    }
}

/// Opens a quick pick. Resolves to the picked item's ID, or `None` when the
/// user dismisses the picker.
pub fn quick_pick(
    workspace: &mut Workspace,
    items: Vec<AddonQuickPickItem>,
    placeholder: Option<String>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> oneshot::Receiver<Option<String>> {
    open(
        workspace,
        PromptKind::QuickPick(items),
        placeholder.unwrap_or_else(|| "Select an item…".to_string()),
        None,
        window,
        cx,
    )
}

/// Opens an input box. Resolves to the entered text, or `None` when the user
/// dismisses it.
pub fn input_box(
    workspace: &mut Workspace,
    prompt: String,
    value: Option<String>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> oneshot::Receiver<Option<String>> {
    open(workspace, PromptKind::InputBox, prompt, value, window, cx)
}

fn open(
    workspace: &mut Workspace,
    kind: PromptKind,
    placeholder: String,
    initial_query: Option<String>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> oneshot::Receiver<Option<String>> {
    let (answer_tx, answer_rx) = oneshot::channel();
    workspace.toggle_modal(window, cx, move |window, cx| {
        let delegate = AddonPromptDelegate {
            prompt: cx.entity().downgrade(),
            kind,
            placeholder: placeholder.into(),
            matches: Vec::new(),
            selected_index: 0,
            query: String::new(),
            answer: Some(answer_tx),
        };
        let picker = cx.new(|cx| {
            let picker = Picker::uniform_list(delegate, window, cx);
            if let Some(query) = &initial_query {
                picker.set_query(query, window, cx);
            }
            picker
        });
        AddonPrompt { picker }
    });
    answer_rx
}

impl PickerDelegate for AddonPromptDelegate {
    type ListItem = ListItem;

    fn name() -> &'static str {
        "addon prompt"
    }

    fn placeholder_text(&self, _window: &mut Window, _cx: &mut App) -> Arc<str> {
        self.placeholder.clone()
    }

    fn match_count(&self) -> usize {
        self.matches.len()
    }

    fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn set_selected_index(
        &mut self,
        ix: usize,
        _window: &mut Window,
        _cx: &mut Context<Picker<Self>>,
    ) {
        self.selected_index = ix;
    }

    fn update_matches(
        &mut self,
        query: String,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Task<()> {
        self.query = query.clone();
        let PromptKind::QuickPick(items) = &self.kind else {
            return Task::ready(());
        };
        let candidates: Vec<_> = items
            .iter()
            .enumerate()
            .map(|(index, item)| StringMatchCandidate::new(index, &item.label))
            .collect();
        let executor = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            let matches = if query.is_empty() {
                candidates
                    .into_iter()
                    .map(|candidate| StringMatch {
                        candidate_id: candidate.id,
                        string: candidate.string,
                        positions: Vec::new(),
                        score: 0.0,
                    })
                    .collect()
            } else {
                match_strings(
                    &candidates,
                    &query,
                    false,
                    true,
                    1000,
                    &Default::default(),
                    executor,
                )
                .await
            };
            this.update(cx, |this, cx| {
                this.delegate.matches = matches;
                this.delegate.selected_index = 0;
                cx.notify();
            })
            .ok();
        })
    }

    fn confirm(&mut self, _secondary: bool, _window: &mut Window, cx: &mut Context<Picker<Self>>) {
        let value = match &self.kind {
            PromptKind::QuickPick(items) => {
                let Some(item) = self
                    .matches
                    .get(self.selected_index)
                    .and_then(|found| items.get(found.candidate_id))
                else {
                    return;
                };
                item.id.clone()
            }
            PromptKind::InputBox => self.query.clone(),
        };
        self.answer(Some(value));
        self.prompt.update(cx, |_, cx| cx.emit(DismissEvent)).ok();
    }

    fn dismissed(&mut self, _window: &mut Window, cx: &mut Context<Picker<Self>>) {
        self.answer(None);
        self.prompt.update(cx, |_, cx| cx.emit(DismissEvent)).ok();
    }

    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        _window: &mut Window,
        _cx: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let PromptKind::QuickPick(items) = &self.kind else {
            return None;
        };
        let found = self.matches.get(ix)?;
        let item = items.get(found.candidate_id)?;
        Some(
            ListItem::new(ix)
                .inset(true)
                .spacing(ListItemSpacing::Sparse)
                .toggle_state(selected)
                .child(HighlightedLabel::new(
                    item.label.clone(),
                    found.positions.clone(),
                ))
                .when_some(item.description.clone(), |row, description| {
                    row.end_slot(Label::new(description).color(Color::Muted))
                }),
        )
    }
}
