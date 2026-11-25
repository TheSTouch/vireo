use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use chrono::{DateTime, Local, Utc};
use editor::{Editor, EditorEvent, EditorMode, MultiBuffer, MultiBufferOffset, SelectionEffects};
use gpui::{
    Action, AnyElement, App, ClipboardEntry, Context, Entity, EventEmitter, FocusHandle, Focusable,
    KeyContext, ScrollHandle, SharedString, Subscription, Task, WeakEntity, Window, prelude::*, px,
};
use language::{Buffer, LanguageRegistry};
use markdown::{Markdown, MarkdownElement, MarkdownFont, MarkdownStyle};
use project::Project;
use serde_json::Value;
use settings::Settings as _;
use ui::{
    CommonAnimationExt as _, ContextMenu, ContextMenuEntry, Divider, HighlightedLabel,
    IconButtonShape, PopoverMenu, Tooltip, prelude::*,
};
use workspace::Workspace;

use crate::conversation::{
    Entry, NoticeKind, PermissionRequest, PermissionStatus, ToolCall, ToolStatus, TurnState,
};
use crate::file_picker::{ProjectEntry, match_entries, project_entries};
use crate::protocol::{OutgoingControlRequest, PermissionDecision, PermissionMode};
use crate::session::{ClaudeSession, ProcessStatus, PromptImage, SessionEvent};
use crate::settings::{ClaudeCodeSettings, spinner_verbs};
use crate::{
    AcceptSuggestion, AllowPermission, AllowPermissionAlways, BlurInput, ConfirmCommand, CycleMode,
    DenyPermission, DismissCommands, FocusInput, FocusLastMessage, Interrupt, NewConversation,
    SelectNextCommand, SelectPreviousCommand, Send, ToggleFocusView, ToggleThinking,
};

gpui::actions!(
    claude_code,
    [
        /// Moves the highlight of the permission or question card down.
        SelectNextOption,
        /// Moves the highlight of the permission or question card up.
        SelectPreviousOption,
        /// Chooses the highlighted option of the permission or question card.
        ConfirmOption,
        /// Sends the answers of Claude's question.
        SubmitAnswers,
        /// Dismisses Claude's question without answering it.
        DismissQuestion,
    ]
);

const ASK_USER_QUESTION: &str = "AskUserQuestion";
const DISMISSED_QUESTION: &str = "The user dismissed the question.";

/// The sizes of the extension's buttons (2.1.330), compared by the drawn
/// glyph rather than the box: its header icons are 20 px boxes whose glyph
/// fills 15 px, the 12/16 of a Codicon drawn at 20 px; its send arrow
/// fills 14 px of 20. Its buttons are 26 to 28 px.
// In rems of 16 px, as rems_from_px is not const.
const HEADER_ICON_SIZE: IconSize = IconSize::Custom(gpui::rems(20. / 16.));
const SEND_ICON_SIZE: IconSize = IconSize::Custom(gpui::rems(18. / 16.));

const SPINNER_INTERVAL: Duration = Duration::from_millis(2500);
const MAX_OUTPUT_LINES: usize = 40;
/// Output lines a tool call shows before it is opened in full: enough to
/// see the result without one call filling the panel.
const PREVIEW_OUTPUT_LINES: usize = 12;
const MAX_DIFF_LINES: usize = 24;

#[derive(Clone, Debug, PartialEq)]
pub enum Attachment {
    /// A file or folder to mention, inserted as `@path`.
    Mention(PathBuf),
    /// Rows of a file, inserted as `@path#start-end`.
    Selection { path: PathBuf, rows: (u32, u32) },
    Image {
        name: SharedString,
        image: PromptImage,
    },
}

/// The open file and selection of the active editor, attached to the next
/// prompt unless the person removes them.
#[derive(Clone, Debug, PartialEq)]
pub struct EditorContext {
    pub path: PathBuf,
    pub selection: Option<(u32, u32, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ViewEvent {
    TitleChanged,
    Unread,
}

pub struct ClaudeCodeView {
    pub session: Entity<ClaudeSession>,
    workspace: WeakEntity<Workspace>,
    language_registry: Arc<LanguageRegistry>,
    input: Entity<Editor>,
    deny_input: Entity<Editor>,
    focus_handle: FocusHandle,
    scroll_handle: ScrollHandle,
    markdown: HashMap<usize, (usize, Entity<Markdown>)>,
    /// Text shown as typed but selectable: your messages and tool calls'
    /// input and output, by entry and part.
    plain_text: HashMap<(usize, PlainPart), (String, Entity<Markdown>)>,
    expanded: HashSet<usize>,
    /// Tool calls the person closed; they start open, as in the extension.
    collapsed: HashSet<usize>,
    /// Tool calls the person opened in full, past the preview's lines.
    full_output: HashSet<usize>,
    /// Timeline rows followed by another, which a rail joins to the next dot.
    rails: HashSet<usize>,
    pub focus_view: bool,
    attachments: Vec<Attachment>,
    editor_context: Option<EditorContext>,
    context_dismissed: bool,
    spinner_verbs: Vec<String>,
    spinner_index: usize,
    banner: Option<SharedString>,
    thinking_enabled: bool,
    /// The list sticks to its latest message until the person scrolls away,
    /// and again once they scroll back to the bottom.
    follow_tail: bool,
    /// The person scrolled since the last frame; the next render works out
    /// from the new offset whether the list still follows its tail.
    scrolled_by_user: bool,
    /// The highlighted row of the "/" command list.
    command_selection: usize,
    /// Escape closed the "/" list; it reopens once the message stops being a command.
    commands_dismissed: bool,
    command_scroll: ScrollHandle,
    /// The "@" mention being typed, whose matches replace the "/" list.
    mention: Option<MentionState>,
    /// The project's files and folders, listed when an "@" list opens.
    mention_candidates: Option<Arc<Vec<ProjectEntry>>>,
    _mention_matches: Option<Task<()>>,
    /// Focus of the card answering the first pending permission request, so
    /// that its options can be chosen with the keyboard.
    card_focus: FocusHandle,
    /// The request the card answers, to start each new one afresh.
    card_request: Option<String>,
    /// The highlighted option of the permission card.
    card_selection: usize,
    /// The answers being given when the pending request is AskUserQuestion.
    question_form: Option<QuestionForm>,
    /// What the last answer to a permission request sent to the CLI.
    #[cfg(test)]
    last_decision: Option<Value>,
    _spinner: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

/// One question of an AskUserQuestion call.
#[derive(Clone, Debug, PartialEq)]
struct Question {
    question: String,
    header: String,
    options: Vec<QuestionOption>,
    multi_select: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct QuestionOption {
    label: String,
    description: String,
}

/// The input of AskUserQuestion: `questions[]` with `question`, `header`,
/// `options[]` (`label`, `description`) and `multiSelect`.
fn parse_questions(input: &Value) -> Vec<Question> {
    let text = |value: &Value, key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    input
        .get("questions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|question| {
            let options = question
                .get("options")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|option| {
                    Some(QuestionOption {
                        label: option.get("label")?.as_str()?.to_string(),
                        description: text(option, "description"),
                    })
                })
                .collect();
            Some(Question {
                question: question.get("question")?.as_str()?.to_string(),
                header: text(question, "header"),
                options,
                multi_select: question
                    .get("multiSelect")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
        })
        .collect()
}

/// The answers the CLI expects back in `updatedInput.answers`: the question's
/// text to the chosen labels joined with ", ", with what was typed in
/// "Other" in place of the word. None while a question has no answer.
fn question_answers(
    questions: &[Question],
    chosen: &[std::collections::BTreeSet<usize>],
    other: &[String],
) -> Option<serde_json::Map<String, Value>> {
    let mut answers = serde_json::Map::new();
    for (index, question) in questions.iter().enumerate() {
        let parts: Vec<&str> = chosen
            .get(index)?
            .iter()
            .filter_map(|&option| match question.options.get(option) {
                Some(option) => Some(option.label.as_str()),
                None => other
                    .get(index)
                    .map(|text| text.trim())
                    .filter(|text| !text.is_empty()),
            })
            .collect();
        if parts.is_empty() {
            return None;
        }
        answers.insert(question.question.clone(), Value::String(parts.join(", ")));
    }
    Some(answers)
}

struct QuestionForm {
    request: PermissionRequest,
    questions: Vec<Question>,
    /// The chosen options of each question; `options.len()` stands for "Other".
    chosen: Vec<std::collections::BTreeSet<usize>>,
    other: Vec<Entity<Editor>>,
    /// The highlighted row, counting every question's options and "Other".
    cursor: usize,
    _subscriptions: Vec<Subscription>,
}

impl QuestionForm {
    fn new(
        request: PermissionRequest,
        window: &mut Window,
        cx: &mut Context<ClaudeCodeView>,
    ) -> Self {
        let questions = parse_questions(&request.request.input);
        let mut subscriptions = Vec::new();
        let other = questions
            .iter()
            .enumerate()
            .map(|(index, _)| {
                let editor = cx.new(|cx| {
                    let mut editor = Editor::single_line(window, cx);
                    editor.set_placeholder_text("Other…", window, cx);
                    editor
                });
                subscriptions.push(cx.subscribe(&editor, move |this, editor, event, cx| {
                    if matches!(event, EditorEvent::BufferEdited) {
                        let typed = !editor.read(cx).text(cx).trim().is_empty();
                        if let Some(form) = this.question_form.as_mut() {
                            form.set_other_typed(index, typed);
                        }
                        cx.notify();
                    }
                }));
                editor
            })
            .collect();
        Self {
            chosen: vec![Default::default(); questions.len()],
            request,
            questions,
            other,
            cursor: 0,
            _subscriptions: subscriptions,
        }
    }

    /// Each row: its question and option, `options.len()` being "Other".
    fn rows(&self) -> Vec<(usize, usize)> {
        self.questions
            .iter()
            .enumerate()
            .flat_map(|(question, item)| {
                (0..=item.options.len()).map(move |option| (question, option))
            })
            .collect()
    }

    fn first_row_of(&self, question: usize) -> usize {
        self.questions
            .iter()
            .take(question)
            .map(|question| question.options.len() + 1)
            .sum()
    }

    fn set_other_typed(&mut self, question: usize, typed: bool) {
        let Some(item) = self.questions.get(question) else {
            return;
        };
        let other = item.options.len();
        let multi_select = item.multi_select;
        let Some(chosen) = self.chosen.get_mut(question) else {
            return;
        };
        if !typed {
            chosen.remove(&other);
        } else if multi_select {
            chosen.insert(other);
        } else {
            chosen.clear();
            chosen.insert(other);
        }
    }

    /// Chooses an option: the only one of a single-choice question, toggled
    /// in a multiple-choice one. Returns whether the question is single-choice.
    fn choose(&mut self, question: usize, option: usize) -> bool {
        let Some(item) = self.questions.get(question) else {
            return false;
        };
        let multi_select = item.multi_select;
        let Some(chosen) = self.chosen.get_mut(question) else {
            return false;
        };
        if multi_select {
            if !chosen.remove(&option) {
                chosen.insert(option);
            }
        } else {
            chosen.clear();
            chosen.insert(option);
        }
        !multi_select
    }

    fn answers(&self, cx: &App) -> Option<serde_json::Map<String, Value>> {
        let other: Vec<String> = self
            .other
            .iter()
            .map(|editor| editor.read(cx).text(cx))
            .collect();
        question_answers(&self.questions, &self.chosen, &other)
    }
}

impl EventEmitter<ViewEvent> for ClaudeCodeView {}

impl ClaudeCodeView {
    pub fn new(
        session: Entity<ClaudeSession>,
        workspace: WeakEntity<Workspace>,
        project: Entity<Project>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings = ClaudeCodeSettings::get_global(cx).clone();
        let language_registry = project.read(cx).languages().clone();
        let input = cx.new(|cx| {
            let buffer = cx.new(|cx| Buffer::local("", cx));
            let buffer = cx.new(|cx| MultiBuffer::singleton(buffer, cx));
            let mut editor = Editor::new(
                EditorMode::AutoHeight {
                    min_lines: 2,
                    max_lines: Some(12),
                },
                buffer,
                None,
                window,
                cx,
            );
            editor.set_placeholder_text(placeholder(None), window, cx);
            editor.set_show_gutter(false, cx);
            editor.set_soft_wrap();
            editor
        });
        let deny_input = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_placeholder_text("Tell Claude what to do instead", window, cx);
            editor
        });
        let mut subscriptions = vec![
            cx.subscribe_in(&session, window, Self::on_session_event),
            cx.subscribe(&input, |this, _, event: &EditorEvent, cx| match event {
                EditorEvent::BufferEdited => {
                    this.refresh_placeholder_later(cx);
                    this.command_selection = 0;
                    this.update_mention(cx);
                }
                EditorEvent::SelectionsChanged { .. } => this.update_mention(cx),
                _ => {}
            }),
            cx.observe_global::<settings::SettingsStore>(|this, cx| {
                let settings = ClaudeCodeSettings::get_global(cx);
                this.spinner_verbs = spinner_verbs(&settings.spinner_verbs);
                cx.notify();
            }),
        ];
        if let Some(workspace) = workspace.upgrade() {
            subscriptions.push(cx.subscribe_in(
                &workspace,
                window,
                |this, workspace, event: &workspace::Event, _, cx| {
                    if matches!(event, workspace::Event::ActiveItemChanged) {
                        this.track_editor_context(workspace, cx);
                    }
                },
            ));
        }
        let this = Self {
            session,
            workspace: workspace.clone(),
            language_registry,
            input,
            deny_input,
            focus_handle: cx.focus_handle(),
            scroll_handle: ScrollHandle::new(),
            markdown: HashMap::new(),
            plain_text: HashMap::new(),
            expanded: HashSet::new(),
            collapsed: HashSet::new(),
            full_output: HashSet::new(),
            rails: HashSet::new(),
            focus_view: settings.focus_view,
            attachments: Vec::new(),
            editor_context: None,
            context_dismissed: false,
            spinner_verbs: spinner_verbs(&settings.spinner_verbs),
            spinner_index: 0,
            banner: None,
            thinking_enabled: true,
            follow_tail: true,
            scrolled_by_user: false,
            command_selection: 0,
            commands_dismissed: false,
            command_scroll: ScrollHandle::new(),
            mention: None,
            mention_candidates: None,
            _mention_matches: None,
            card_focus: cx.focus_handle(),
            card_request: None,
            card_selection: 0,
            question_form: None,
            #[cfg(test)]
            last_decision: None,
            _spinner: None,
            _subscriptions: subscriptions,
        };
        // The view is often built inside an update of the workspace, which
        // cannot be read until that update ends.
        cx.spawn(async move |this, cx| {
            let Some(workspace) = workspace.upgrade() else {
                return;
            };
            this.update(cx, |this, cx| this.track_editor_context(&workspace, cx))
                .ok();
        })
        .detach();
        // Tests drive the session themselves rather than starting a real `claude`.
        this.session.update(cx, |session, cx| {
            if cx.is_test() {
                return;
            }
            if let Err(error) = session.start(cx) {
                session.conversation.push_notice(
                    NoticeKind::Error,
                    format!("{error:#}"),
                    Utc::now(),
                );
            }
        });
        this
    }

    pub fn set_input_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input
            .update(cx, |input, cx| input.set_text(text, window, cx));
    }

    /// "Fork conversation from here" on a prompt: a new tab with the session
    /// up to just before it and the prompt in the input. With `rewind`, the
    /// files are first put back as they were before that prompt.
    fn fork_from(
        &mut self,
        index: usize,
        rewind: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let session = self.session.read(cx);
        let Some(Entry::User(user)) = session.conversation.entries.get(index) else {
            return;
        };
        let Some(session_id) = session.session_id().map(str::to_string) else {
            return;
        };
        let prompt = strip_context_preamble(&user.text).to_string();
        let resume_at = user.previous_uuid.clone();
        let user_message_id = user.uuid.clone();
        let entries: Vec<Entry> = session.conversation.entries[..index]
            .iter()
            .filter(|entry| {
                !matches!(entry, Entry::Permission(request) if request.status == PermissionStatus::Pending)
            })
            .cloned()
            .collect();
        if rewind && let Some(user_message_id) = user_message_id {
            self.rewind_code_to(user_message_id, cx);
        }
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        workspace.update(cx, |workspace, cx| {
            crate::panel::open_fork(
                workspace, session_id, resume_at, entries, prompt, window, cx,
            );
        });
    }

    /// "Rewind code to here": undoes the file changes Claude made since this
    /// prompt, using the CLI's file checkpoints.
    fn rewind_code_to(&mut self, user_message_id: String, cx: &mut Context<Self>) {
        let task = self.session.update(cx, |session, cx| {
            session.request(OutgoingControlRequest::RewindFiles { user_message_id }, cx)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                let (kind, text) = match result {
                    Ok(response) => describe_rewind(&response),
                    Err(error) => (
                        NoticeKind::Error,
                        format!("Could not rewind the code: {error:#}"),
                    ),
                };
                this.session.update(cx, |session, _| {
                    session.conversation.push_notice(kind, text, Utc::now())
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn title(&self, cx: &App) -> SharedString {
        let conversation = &self.session.read(cx).conversation;
        if let Some(title) = conversation
            .session_id
            .as_deref()
            .and_then(|session_id| crate::sessions::custom_title(session_id, cx))
        {
            return title.into();
        }
        if let Some(title) = &conversation.title {
            return title.clone().into();
        }
        conversation
            .entries
            .iter()
            .find_map(|entry| match entry {
                Entry::User(user) if !user.text.trim().is_empty() => Some(truncate(
                    user.text.trim().lines().next().unwrap_or_default(),
                    48,
                )),
                _ => None,
            })
            .map(SharedString::from)
            .unwrap_or_else(|| "Claude Code".into())
    }

    pub fn input_focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }

    pub fn is_running(&self, cx: &App) -> bool {
        self.session.read(cx).conversation.turn != TurnState::Idle
    }

    pub fn set_banner(&mut self, banner: Option<SharedString>, cx: &mut Context<Self>) {
        self.banner = banner;
        cx.notify();
    }

    pub fn add_attachment(&mut self, attachment: Attachment, cx: &mut Context<Self>) {
        if !self.attachments.contains(&attachment) {
            self.attachments.push(attachment);
        }
        cx.notify();
    }

    /// Inserts `@path` or `@path#start-end` at the cursor of the message box.
    pub fn insert_mention(
        &mut self,
        path: &Path,
        rows: Option<(u32, u32)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mention = mention_text(&self.relative_path(path, cx), rows);
        self.input.update(cx, |input, cx| {
            input.insert(&format!("{mention} "), window, cx);
        });
        window.focus(&self.input.focus_handle(cx), cx);
    }

    fn relative_path(&self, path: &Path, cx: &App) -> String {
        let cwd = &self.session.read(cx).cwd;
        path.strip_prefix(cwd)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned()
    }

    fn track_editor_context(&mut self, workspace: &Entity<Workspace>, cx: &mut Context<Self>) {
        let editor = workspace
            .read(cx)
            .active_item(cx)
            .and_then(|item| item.act_as::<Editor>(cx));
        let Some(editor) = editor else {
            return;
        };
        self._subscriptions.truncate(4);
        self._subscriptions.push(
            cx.subscribe(&editor, |this, editor, event: &EditorEvent, cx| {
                if matches!(event, EditorEvent::SelectionsChanged { .. }) {
                    this.update_editor_context(&editor, cx);
                }
            }),
        );
        self.update_editor_context(&editor, cx);
    }

    fn update_editor_context(&mut self, editor: &Entity<Editor>, cx: &mut Context<Self>) {
        let Some(selection) = claude_ide::editor_selection(editor.read(cx), cx) else {
            return;
        };
        let selection_context = (!selection.is_empty())
            .then(|| (selection.rows.0, selection.rows.1, selection.text.clone()));
        let context = EditorContext {
            path: selection.file_path,
            selection: selection_context,
        };
        if self.editor_context.as_ref() != Some(&context) {
            self.editor_context = Some(context);
            self.context_dismissed = false;
            cx.notify();
        }
    }

    fn effective_context(&self, cx: &App) -> Option<EditorContext> {
        if self.context_dismissed {
            return None;
        }
        let settings = ClaudeCodeSettings::get_global(cx);
        let mut context = self.editor_context.clone()?;
        if !settings.attach_selection {
            context.selection = None;
        }
        if !settings.attach_open_file && context.selection.is_none() {
            return None;
        }
        Some(context)
    }

    fn on_session_event(
        &mut self,
        _: &Entity<ClaudeSession>,
        event: &SessionEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SessionEvent::Updated => {
                self.sync_card(window, cx);
                self.ensure_spinner(cx);
                cx.emit(ViewEvent::TitleChanged);
                self.refresh_placeholder(window, cx);
            }
            SessionEvent::TurnFinished => {
                if !self.focus_handle.contains_focused(window, cx) {
                    cx.emit(ViewEvent::Unread);
                }
            }
            SessionEvent::PermissionRequested { request_id } => {
                let typing_here = self.focus_handle.contains_focused(window, cx)
                    || self.input.focus_handle(cx).is_focused(window);
                self.sync_card(window, cx);
                if typing_here && self.card_request.is_some() {
                    window.focus(&self.card_focus, cx);
                }
                self.open_proposed_diff(request_id.clone(), window, cx);
            }
            SessionEvent::Exited => {}
        }
        cx.notify();
    }

    fn is_scrolled_to_bottom(&self) -> bool {
        let max = self.scroll_handle.max_offset().y;
        let offset = -self.scroll_handle.offset().y;
        offset >= max - px(24.)
    }

    /// Scrolls the list to its bottom on every frame while it follows its
    /// tail. A one-off scroll is not enough: a restored transcript is laid out
    /// before any session event, and replies grow after their first layout
    /// as their Markdown is parsed in the background.
    fn update_follow_tail(&mut self) {
        if std::mem::take(&mut self.scrolled_by_user) {
            self.follow_tail = self.is_scrolled_to_bottom();
        }
        if self.follow_tail {
            self.scroll_handle.scroll_to_bottom();
        }
    }

    fn shows_jump_to_bottom(&self) -> bool {
        !self.follow_tail && !self.is_scrolled_to_bottom()
    }

    fn jump_to_bottom(&mut self, cx: &mut Context<Self>) {
        self.follow_tail = true;
        self.scroll_handle.scroll_to_bottom();
        cx.notify();
    }

    fn ensure_spinner(&mut self, cx: &mut Context<Self>) {
        let running = self.is_running(cx);
        if !running {
            self._spinner = None;
            return;
        }
        if self._spinner.is_some() {
            return;
        }
        self._spinner = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(SPINNER_INTERVAL).await;
                let keep_going = this
                    .update(cx, |this, cx| {
                        this.spinner_index = this.spinner_index.wrapping_add(1);
                        cx.notify();
                        this.is_running(cx)
                    })
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        }));
    }

    fn refresh_placeholder_later(&mut self, cx: &mut Context<Self>) {
        cx.notify();
    }

    fn refresh_placeholder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let suggestion = if ClaudeCodeSettings::get_global(cx).prompt_suggestions {
            self.session.read(cx).conversation.prompt_suggestion.clone()
        } else {
            None
        };
        self.input.update(cx, |input, cx| {
            input.set_placeholder_text(placeholder(suggestion.as_deref()), window, cx);
        });
    }

    fn send(&mut self, _: &Send, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.input.read(cx).text(cx);
        if text.trim().is_empty() && self.attachments.is_empty() {
            return;
        }
        let mut prompt = String::new();
        let mut images = Vec::new();
        for attachment in self.attachments.drain(..) {
            match attachment {
                Attachment::Mention(path) => {
                    let relative = path
                        .strip_prefix(&self.session.read(cx).cwd)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .into_owned();
                    prompt.push_str(&mention_text(&relative, None));
                    prompt.push(' ');
                }
                Attachment::Selection { path, rows } => {
                    let relative = path
                        .strip_prefix(&self.session.read(cx).cwd)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .into_owned();
                    prompt.push_str(&mention_text(&relative, Some(rows)));
                    prompt.push(' ');
                }
                Attachment::Image { image, .. } => images.push(image),
            }
        }
        prompt.push_str(text.trim());
        let context = self.effective_context(cx);
        let full_prompt = match context {
            Some(context) => format!("{}{prompt}", context_preamble(&context)),
            None => prompt,
        };
        let result = self.session.update(cx, |session, cx| {
            session.send_prompt(full_prompt, images, cx)
        });
        if result.is_ok() {
            self.input.update(cx, |input, cx| input.clear(window, cx));
            self.context_dismissed = true;
            if ClaudeCodeSettings::get_global(cx).scroll_to_bottom_on_send {
                self.scroll_handle.scroll_to_bottom();
                self.follow_tail = true;
            }
        }
        self.ensure_spinner(cx);
        cx.notify();
    }

    /// The suggested prompt the empty message box shows in grey, if any.
    fn shown_suggestion(&self, cx: &App) -> Option<String> {
        if !ClaudeCodeSettings::get_global(cx).prompt_suggestions
            || !self.input.read(cx).text(cx).is_empty()
        {
            return None;
        }
        self.session.read(cx).conversation.prompt_suggestion.clone()
    }

    fn accept_suggestion(
        &mut self,
        _: &AcceptSuggestion,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(suggestion) = self.shown_suggestion(cx) else {
            cx.propagate();
            return;
        };
        self.input.update(cx, |input, cx| {
            input.set_text(suggestion, window, cx);
            input.move_to_end(&editor::actions::MoveToEnd, window, cx);
        });
    }

    fn interrupt(&mut self, _: &Interrupt, _: &mut Window, cx: &mut Context<Self>) {
        if !self.is_running(cx) {
            cx.propagate();
            return;
        }
        self.session.update(cx, |session, cx| session.interrupt(cx));
    }

    fn toggle_focus_view(&mut self, _: &ToggleFocusView, _: &mut Window, cx: &mut Context<Self>) {
        self.focus_view = !self.focus_view;
        cx.notify();
    }

    fn toggle_thinking(&mut self, _: &ToggleThinking, _: &mut Window, cx: &mut Context<Self>) {
        self.thinking_enabled = !self.thinking_enabled;
        let tokens = if self.thinking_enabled { None } else { Some(0) };
        self.session.update(cx, |session, cx| {
            session.set_max_thinking_tokens(tokens, cx)
        });
        cx.notify();
    }

    fn focus_input(&mut self, _: &FocusInput, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.input.focus_handle(cx), cx);
    }

    fn blur_input(&mut self, _: &BlurInput, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(workspace) = self.workspace.upgrade() {
            let pane = workspace.read(cx).active_pane().clone();
            window.focus(&pane.focus_handle(cx), cx);
        }
    }

    fn focus_last_message(
        &mut self,
        _: &FocusLastMessage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.follow_tail = true;
        self.scroll_handle.scroll_to_bottom();
        window.focus(&self.focus_handle, cx);
    }

    fn cycle_mode(&mut self, _: &CycleMode, _: &mut Window, cx: &mut Context<Self>) {
        let modes = self.available_modes(cx);
        let current = self.session.read(cx).conversation.permission_mode;
        let index = current
            .and_then(|current| modes.iter().position(|mode| *mode == current))
            .map_or(0, |index| (index + 1) % modes.len().max(1));
        if let Some(mode) = modes.get(index).copied() {
            self.session
                .update(cx, |session, cx| session.set_permission_mode(mode, cx));
        }
    }

    fn available_modes(&self, cx: &App) -> Vec<PermissionMode> {
        available_modes(ClaudeCodeSettings::get_global(cx).allow_dangerously_skip_permissions)
    }

    fn pending_permission(&self, cx: &App) -> Option<PermissionRequest> {
        self.session
            .read(cx)
            .conversation
            .pending_permissions()
            .next()
            .cloned()
    }

    fn allow_permission(&mut self, _: &AllowPermission, _: &mut Window, cx: &mut Context<Self>) {
        let Some(request) = self.pending_permission(cx) else {
            cx.propagate();
            return;
        };
        self.answer_permission(&request, Answer::Allow, cx);
    }

    fn allow_permission_always(
        &mut self,
        _: &AllowPermissionAlways,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(request) = self.pending_permission(cx) else {
            cx.propagate();
            return;
        };
        self.answer_permission(&request, Answer::AllowAlways, cx);
    }

    fn deny_permission(&mut self, _: &DenyPermission, window: &mut Window, cx: &mut Context<Self>) {
        let Some(request) = self.pending_permission(cx) else {
            cx.propagate();
            return;
        };
        let message = self.deny_input.read(cx).text(cx);
        self.deny_input
            .update(cx, |input, cx| input.clear(window, cx));
        self.answer_permission(&request, Answer::Deny(message), cx);
    }

    pub fn answer_permission(
        &mut self,
        request: &PermissionRequest,
        answer: Answer,
        cx: &mut Context<Self>,
    ) {
        let decision = match answer {
            Answer::Allow => PermissionDecision::Allow {
                updated_input: request.request.input.clone(),
                updated_permissions: Vec::new(),
            },
            Answer::AllowWithInput(input) => PermissionDecision::Allow {
                updated_input: input,
                updated_permissions: Vec::new(),
            },
            Answer::AllowAlways => PermissionDecision::Allow {
                updated_input: request.request.input.clone(),
                updated_permissions: request.request.permission_suggestions.clone(),
            },
            Answer::Dismiss(message) => PermissionDecision::Deny {
                message,
                interrupt: false,
            },
            Answer::Deny(message) => {
                let message = message.trim().to_string();
                PermissionDecision::Deny {
                    interrupt: message.is_empty(),
                    message: if message.is_empty() {
                        "The user doesn't want to proceed with this tool use. The tool use was rejected.".into()
                    } else {
                        format!(
                            "The user doesn't want to proceed with this tool use. They said: {message}"
                        )
                    },
                }
            }
        };
        let request_id = request.request_id.clone();
        #[cfg(test)]
        {
            self.last_decision = serde_json::to_value(&decision).ok();
        }
        self.session.update(cx, |session, cx| {
            session.respond_permission(&request_id, decision, cx)
        });
        crate::diff_review::close_matching(cx, |diff| diff.tab_name == request_id);
    }

    /// Shows an edit Claude asks permission for as a diff to review. The
    /// answer in the diff answers the permission request.
    fn open_proposed_diff(
        &mut self,
        request_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(request) = self
            .session
            .read(cx)
            .conversation
            .pending_permissions()
            .find(|request| request.request_id == request_id)
            .cloned()
        else {
            return;
        };
        let tool_name = request.request.tool_name.clone();
        if !matches!(tool_name.as_str(), "Edit" | "MultiEdit" | "Write") {
            return;
        }
        let Some(path) =
            crate::diff_review::path_for_tool(&request.request.input).map(Path::to_path_buf)
        else {
            return;
        };
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let current = std::fs::read_to_string(&path).unwrap_or_default();
        let proposed = match crate::diff_review::proposed_contents(
            &tool_name,
            &request.request.input,
            &current,
        ) {
            Ok(proposed) => proposed,
            Err(error) => {
                log::debug!("not showing a diff for {tool_name}: {error:#}");
                return;
            }
        };
        let opened = crate::diff_review::ProposedDiff::open(
            &workspace,
            claude_ide::DiffRequest {
                old_file_path: path.clone(),
                new_file_path: path,
                new_file_contents: proposed.clone(),
                tab_name: request_id,
            },
            window,
            cx,
        );
        let receiver = match opened {
            Ok((_, receiver)) => receiver,
            Err(error) => {
                log::warn!("failed to open the proposed diff: {error:#}");
                return;
            }
        };
        cx.spawn(async move |this, cx| {
            let Ok(outcome) = receiver.await else {
                return;
            };
            this.update(cx, |this, cx| {
                let still_pending = this
                    .session
                    .read(cx)
                    .conversation
                    .pending_permissions()
                    .any(|pending| pending.request_id == request.request_id);
                if !still_pending {
                    return;
                }
                let answer = match outcome {
                    claude_ide::DiffOutcome::Saved(text)
                        if text == crate::diff_review::normalize(&proposed) =>
                    {
                        Answer::Allow
                    }
                    claude_ide::DiffOutcome::Saved(text) => {
                        Answer::AllowWithInput(crate::diff_review::updated_input(
                            &tool_name,
                            &request.request.input,
                            &current,
                            &text,
                        ))
                    }
                    claude_ide::DiffOutcome::Rejected => Answer::Deny(String::new()),
                };
                this.answer_permission(&request, answer, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Starts the card afresh when the first pending request changes, and
    /// gives the focus back to the message box once nothing is pending.
    fn sync_card(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.pending_permission(cx);
        let active_id = active.as_ref().map(|request| request.request_id.clone());
        if active_id == self.card_request {
            return;
        }
        let card_had_focus = self.card_focus.contains_focused(window, cx);
        self.card_request = active_id;
        self.card_selection = 0;
        self.question_form = active
            .filter(|request| request.request.tool_name == ASK_USER_QUESTION)
            .map(|request| QuestionForm::new(request, window, cx));
        if card_had_focus && self.card_request.is_none() {
            window.focus(&self.input.focus_handle(cx), cx);
        }
    }

    fn permission_options(request: &PermissionRequest) -> Vec<(String, PermissionChoice)> {
        let tool = &request.request;
        let mut options = vec![("Yes".to_string(), PermissionChoice::Allow)];
        if !tool.permission_suggestions.is_empty() {
            let always = if is_edit_tool(&tool.tool_name) {
                "Yes, allow all edits this session"
            } else {
                "Yes, and don't ask again"
            };
            options.push((always.to_string(), PermissionChoice::AllowAlways));
        }
        options.push((
            "No, and tell Claude what to do differently".to_string(),
            PermissionChoice::Deny,
        ));
        options
    }

    fn choose_permission_option(
        &mut self,
        option: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(request) = self.pending_permission(cx) else {
            return;
        };
        let Some((_, choice)) = Self::permission_options(&request).get(option).cloned() else {
            return;
        };
        let answer = match choice {
            PermissionChoice::Allow => Answer::Allow,
            PermissionChoice::AllowAlways => Answer::AllowAlways,
            PermissionChoice::Deny => {
                let message = self.deny_input.read(cx).text(cx);
                self.deny_input
                    .update(cx, |input, cx| input.clear(window, cx));
                Answer::Deny(message)
            }
        };
        self.answer_permission(&request, answer, cx);
    }

    fn move_card_selection(&mut self, step: isize, cx: &mut Context<Self>) {
        if let Some(form) = self.question_form.as_mut() {
            let count = form.rows().len().max(1) as isize;
            form.cursor = (form.cursor as isize + step).rem_euclid(count) as usize;
        } else if let Some(request) = self.pending_permission(cx) {
            let count = Self::permission_options(&request).len().max(1) as isize;
            self.card_selection = (self.card_selection as isize + step).rem_euclid(count) as usize;
        } else {
            cx.propagate();
            return;
        }
        cx.notify();
    }

    fn select_next_option(&mut self, _: &SelectNextOption, _: &mut Window, cx: &mut Context<Self>) {
        self.move_card_selection(1, cx);
    }

    fn select_previous_option(
        &mut self,
        _: &SelectPreviousOption,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_card_selection(-1, cx);
    }

    fn confirm_option(&mut self, _: &ConfirmOption, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(form) = self.question_form.as_ref() {
            let Some((question, option)) = form.rows().get(form.cursor).copied() else {
                return;
            };
            self.choose_question_option(question, option, window, cx);
        } else if self.pending_permission(cx).is_some() {
            self.choose_permission_option(self.card_selection, window, cx);
        } else {
            cx.propagate();
        }
    }

    /// Chooses an option of a question, or goes to its "Other" field. After a
    /// single choice the highlight moves on to the next question.
    fn choose_question_option(
        &mut self,
        question: usize,
        option: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(form) = self.question_form.as_mut() else {
            return;
        };
        let Some(item) = form.questions.get(question) else {
            return;
        };
        if option >= item.options.len() {
            form.cursor = form.first_row_of(question) + item.options.len();
            if let Some(editor) = form.other.get(question) {
                window.focus(&editor.focus_handle(cx), cx);
            }
            cx.notify();
            return;
        }
        let single = form.choose(question, option);
        form.cursor = if single && question + 1 < form.questions.len() {
            form.first_row_of(question + 1)
        } else {
            form.first_row_of(question) + option
        };
        if single && let Some(editor) = form.other.get(question) {
            editor.update(cx, |editor, cx| editor.clear(window, cx));
        }
        window.focus(&self.card_focus, cx);
        cx.notify();
    }

    fn submit_answers(&mut self, _: &SubmitAnswers, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = self.question_form.as_ref() else {
            cx.propagate();
            return;
        };
        let Some(answers) = form.answers(cx) else {
            return;
        };
        let request = form.request.clone();
        let mut input = request.request.input.clone();
        if let Some(fields) = input.as_object_mut() {
            fields.insert("answers".to_string(), Value::Object(answers));
        }
        self.answer_permission(&request, Answer::AllowWithInput(input), cx);
        self.sync_card(window, cx);
    }

    fn dismiss_question(
        &mut self,
        _: &DismissQuestion,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(form) = self.question_form.as_ref() else {
            cx.propagate();
            return;
        };
        let request = form.request.clone();
        self.answer_permission(
            &request,
            Answer::Dismiss(DISMISSED_QUESTION.to_string()),
            cx,
        );
        self.sync_card(window, cx);
    }

    /// Number keys choose an option of the card: 1 to 3 on a permission card,
    /// and on a question card the options of the highlighted question, the
    /// one after the last being "Other".
    fn card_key_down(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.modified() {
            return;
        }
        let Some(number) = keystroke
            .key
            .parse::<usize>()
            .ok()
            .filter(|number| *number >= 1)
        else {
            return;
        };
        let option = number - 1;
        if let Some(form) = self.question_form.as_ref() {
            let Some((question, _)) = form.rows().get(form.cursor).copied() else {
                return;
            };
            let options = form
                .questions
                .get(question)
                .map_or(0, |item| item.options.len());
            if option <= options {
                cx.stop_propagation();
                self.choose_question_option(question, option, window, cx);
            }
        } else if let Some(request) = self.pending_permission(cx)
            && option < Self::permission_options(&request).len()
        {
            cx.stop_propagation();
            self.choose_permission_option(option, window, cx);
        }
    }

    fn paste(&mut self, _: &editor::actions::Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            cx.propagate();
            return;
        };
        let images: Vec<_> = item
            .entries()
            .iter()
            .filter_map(|entry| match entry {
                ClipboardEntry::Image(image) => Some(image.clone()),
                _ => None,
            })
            .collect();
        if images.is_empty() {
            cx.propagate();
            return;
        }
        for (index, image) in images.into_iter().enumerate() {
            let count = self
                .attachments
                .iter()
                .filter(|attachment| matches!(attachment, Attachment::Image { .. }))
                .count();
            self.attachments.push(Attachment::Image {
                name: format!("Image {}", count + index + 1).into(),
                image: PromptImage {
                    media_type: image.format.mime_type().to_string(),
                    base64: base64::engine::general_purpose::STANDARD.encode(&image.bytes),
                },
            });
        }
        cx.notify();
    }

    fn new_conversation_requested(
        &mut self,
        _: &NewConversation,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The panel or tab that hosts this view replaces the session.
        cx.propagate();
    }

    fn markdown_for(
        &mut self,
        index: usize,
        text: &str,
        cx: &mut Context<Self>,
    ) -> Entity<Markdown> {
        if let Some((length, markdown)) = self.markdown.get(&index)
            && *length == text.len()
        {
            return markdown.clone();
        }
        let markdown = match self.markdown.get(&index) {
            Some((_, markdown)) => {
                let markdown = markdown.clone();
                markdown.update(cx, |markdown, cx| markdown.replace(text.to_string(), cx));
                markdown
            }
            None => {
                let language_registry = self.language_registry.clone();
                let text: SharedString = text.to_string().into();
                cx.new(|cx| Markdown::new(text, Some(language_registry), None, cx))
            }
        };
        self.markdown.insert(index, (text.len(), markdown.clone()));
        markdown
    }

    /// Text drawn exactly as written, newlines kept and nothing taken as
    /// Markdown, in an element that can be selected and copied.
    fn plain_text_for(
        &mut self,
        index: usize,
        part: PlainPart,
        text: &str,
        cx: &mut Context<Self>,
    ) -> Entity<Markdown> {
        if let Some((cached, markdown)) = self.plain_text.get(&(index, part))
            && cached == text
        {
            return markdown.clone();
        }
        let markdown = cx.new(|cx| Markdown::new_text(text.to_string().into(), cx));
        self.plain_text
            .insert((index, part), (text.to_string(), markdown.clone()));
        markdown
    }

    fn copy_user_message(&self, index: usize, cx: &mut App) {
        if let Some(Entry::User(user)) = self.session.read(cx).conversation.entries.get(index) {
            let text = strip_context_preamble(&user.text).to_string();
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        }
    }

    fn render_entries(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let settings = ClaudeCodeSettings::get_global(cx).clone();
        let entries = self.session.read(cx).conversation.entries.clone();
        let mut elements = Vec::with_capacity(entries.len());
        let mut previous_day = None;
        let focus_view = self.focus_view;
        let visible = |entry: &Entry| {
            !(focus_view && entry.is_activity())
                && !matches!(entry, Entry::AssistantText(text) if text.parent_tool_use_id.is_some())
                && !matches!(entry, Entry::ToolCall(call) if call.parent_tool_use_id.is_some())
        };
        let shown: Vec<usize> = entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| visible(entry))
            .map(|(index, _)| index)
            .collect();
        self.rails = shown
            .windows(2)
            .filter(|pair| {
                !matches!(entries.get(pair[0]), Some(Entry::User(_)))
                    && !matches!(entries.get(pair[1]), Some(Entry::User(_)))
            })
            .map(|pair| pair[0])
            .collect();
        for (index, entry) in entries.iter().enumerate() {
            if !visible(entry) {
                continue;
            }
            let local: DateTime<Local> = entry.timestamp().with_timezone(&Local);
            let day = local.date_naive();
            if settings.show_message_timestamps
                && previous_day.is_some_and(|previous| previous != day)
            {
                elements.push(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .child(Divider::horizontal())
                        .child(
                            Label::new(local.format("%A %-d %B").to_string())
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        )
                        .child(Divider::horizontal())
                        .into_any_element(),
                );
            }
            previous_day = Some(day);
            let timestamp = settings
                .show_message_timestamps
                .then(|| local.format("%-I:%M %p").to_string());
            let element = match entry {
                Entry::User(user) => self.render_user(index, user, timestamp, window, cx),
                Entry::AssistantText(text) => {
                    let markdown = self.markdown_for(index, &text.text, cx);
                    timeline_row(
                        index,
                        Color::Default,
                        false,
                        MarkdownElement::new(markdown, message_markdown_style(window, cx))
                            .into_any_element(),
                        timestamp,
                        self.rail(index, cx),
                    )
                }
                Entry::Thinking(thinking) => self.render_thinking(index, thinking, cx),
                Entry::ToolCall(call) => self.render_tool_call(index, call, window, cx),
                Entry::Permission(request) => self.render_permission(index, request, window, cx),
                Entry::Notice(notice) => timeline_row(
                    index,
                    if notice.kind == NoticeKind::Error {
                        Color::Error
                    } else {
                        Color::Muted
                    },
                    false,
                    Label::new(notice.text.clone())
                        .size(LabelSize::Small)
                        .color(if notice.kind == NoticeKind::Error {
                            Color::Error
                        } else {
                            Color::Muted
                        })
                        .into_any_element(),
                    None,
                    self.rail(index, cx),
                ),
            };
            elements.push(element);
        }
        elements
    }

    fn render_user(
        &mut self,
        index: usize,
        user: &crate::conversation::UserEntry,
        timestamp: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let text = self.plain_text_for(
            index,
            PlainPart::Message,
            strip_context_preamble(&user.text),
            cx,
        );
        let colors = cx.theme().colors();
        let group: SharedString = format!("claude-user-{index}").into();
        let copy_button = IconButton::new(("copy-message", index), IconName::Copy)
            .icon_size(IconSize::XSmall)
            .icon_color(Color::Muted)
            .tooltip(Tooltip::text("Copy message"))
            .on_click(cx.listener(move |this, _, _, cx| this.copy_user_message(index, cx)));
        let history_actions =
            user.uuid
                .clone()
                .filter(|_| !self.is_running(cx))
                .map(|user_message_id| {
                    h_flex()
                        .gap_0p5()
                        .child(
                            IconButton::new(("fork", index), IconName::GitBranch)
                                .icon_size(IconSize::XSmall)
                                .icon_color(Color::Muted)
                                .tooltip(Tooltip::text("Fork conversation from here"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.fork_from(index, false, window, cx)
                                })),
                        )
                        .child(
                            IconButton::new(("rewind", index), IconName::RotateCcw)
                                .icon_size(IconSize::XSmall)
                                .icon_color(Color::Muted)
                                .tooltip(Tooltip::text("Rewind code to here"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.rewind_code_to(user_message_id.clone(), cx)
                                })),
                        )
                        .child(
                            IconButton::new(("fork-rewind", index), IconName::GitBranchPlus)
                                .icon_size(IconSize::XSmall)
                                .icon_color(Color::Muted)
                                .tooltip(Tooltip::text("Fork conversation and rewind code"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.fork_from(index, true, window, cx)
                                })),
                        )
                });
        let actions = h_flex()
            .justify_end()
            .gap_0p5()
            .visible_on_hover(group.clone())
            .child(copy_button)
            .children(history_actions);
        v_flex()
            .id(("user", index))
            .group(group)
            .w_full()
            .gap_0p5()
            .when_some(timestamp, |this, timestamp| {
                this.child(
                    h_flex().justify_end().child(
                        Label::new(timestamp)
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    ),
                )
            })
            .child(
                v_flex()
                    .w_full()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .border_1()
                    .border_color(colors.border)
                    .bg(colors.editor_background)
                    .child(MarkdownElement::new(
                        text,
                        message_markdown_style(window, cx),
                    ))
                    .when(user.images > 0, |this| {
                        this.child(
                            h_flex()
                                .gap_1()
                                .child(
                                    Icon::new(IconName::Image)
                                        .size(IconSize::XSmall)
                                        .color(Color::Muted),
                                )
                                .child(
                                    Label::new(format!(
                                        "{} image{}",
                                        user.images,
                                        if user.images == 1 { "" } else { "s" }
                                    ))
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                                ),
                        )
                    })
                    .when(user.queued, |this| {
                        this.child(
                            Label::new("Queued")
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        )
                    }),
            )
            .child(actions)
            .into_any_element()
    }

    fn render_thinking(
        &self,
        index: usize,
        thinking: &crate::conversation::ThinkingEntry,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let expanded = self.expanded.contains(&index);
        let label = match thinking.duration_seconds() {
            Some(seconds) => format!("Thought for {seconds}s"),
            None => "Thinking…".to_string(),
        };
        let header = h_flex()
            .id(("thinking", index))
            .gap_1()
            .cursor_pointer()
            .child(Label::new(label).size(LabelSize::Small).color(Color::Muted))
            .child(
                Icon::new(if expanded {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .size(IconSize::XSmall)
                .color(Color::Muted),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_expanded(index, cx)));
        let content =
            v_flex()
                .gap_1()
                .child(header)
                .when(expanded && !thinking.text.is_empty(), |this| {
                    this.child(
                        Label::new(thinking.text.clone())
                            .size(LabelSize::Small)
                            .color(Color::Muted)
                            .italic(),
                    )
                });
        timeline_row(
            index,
            Color::Muted,
            false,
            content.into_any_element(),
            None,
            self.rail(index, cx),
        )
    }

    fn rail(&self, index: usize, cx: &App) -> Option<gpui::Hsla> {
        self.rails
            .contains(&index)
            .then(|| cx.theme().colors().border_variant)
    }

    fn toggle_expanded(&mut self, index: usize, cx: &mut Context<Self>) {
        if !self.expanded.remove(&index) {
            self.expanded.insert(index);
        }
        cx.notify();
    }

    fn render_tool_call(
        &mut self,
        index: usize,
        call: &ToolCall,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let input_text =
            self.plain_text_for(index, PlainPart::ToolInput, &tool_input_text(call), cx);
        let expanded = !self.collapsed.contains(&index);
        let (dot, running) = match call.status {
            ToolStatus::Streaming | ToolStatus::Running => (Color::Accent, true),
            ToolStatus::Succeeded => (Color::Success, false),
            ToolStatus::Failed => (Color::Error, false),
        };
        let diff = edit_diff(call);
        let summary = match call.file_path() {
            Some(path) => self.relative_path(Path::new(path), cx),
            None => call.summary(),
        };
        let header = h_flex()
            .id(("tool", index))
            .gap_1p5()
            .cursor_pointer()
            .overflow_hidden()
            .child(
                Label::new(display_tool_name(&call.name))
                    .size(LabelSize::Small)
                    .weight(gpui::FontWeight::SEMIBOLD),
            )
            .child(
                Label::new(summary)
                    .size(LabelSize::Small)
                    .color(if call.file_path().is_some() {
                        Color::Accent
                    } else {
                        Color::Muted
                    })
                    .truncate(),
            )
            .when_some(diff.as_ref(), |this, diff| {
                let (added, removed) = diff_counts(diff);
                this.child(
                    Label::new(format!("+{added}"))
                        .size(LabelSize::XSmall)
                        .color(Color::Created),
                )
                .child(
                    Label::new(format!("−{removed}"))
                        .size(LabelSize::XSmall)
                        .color(Color::Deleted),
                )
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.collapsed.remove(&index) {
                    this.collapsed.insert(index);
                }
                cx.notify();
            }));
        let full_output = self.full_output.contains(&index);
        let output = call
            .result
            .as_deref()
            .filter(|result| !result.trim().is_empty())
            // An edit that worked says so in its diff; its result only repeats it.
            .filter(|_| diff.is_none() || call.status == ToolStatus::Failed)
            .map(|result| {
                let limit = if full_output {
                    MAX_OUTPUT_LINES
                } else {
                    PREVIEW_OUTPUT_LINES
                };
                (
                    truncate_lines(result, limit),
                    !full_output && result.lines().count() > PREVIEW_OUTPUT_LINES,
                )
            });
        let output_text = output.map(|(text, more)| {
            (
                self.plain_text_for(index, PlainPart::ToolOutput, &text, cx),
                more,
            )
        });
        let block_style = tool_block_markdown_style(window, cx);
        let colors = cx.theme().colors();
        let status_colors = cx.theme().status();
        let block = |label: &'static str, text: Entity<Markdown>| {
            h_flex()
                .items_start()
                .gap_2()
                .px_2()
                .py_1()
                .child(
                    div().w(px(28.)).flex_none().child(
                        Label::new(label)
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(MarkdownElement::new(text, block_style.clone())),
                )
        };
        let body = v_flex()
            .mt_1()
            .rounded_md()
            .border_1()
            .border_color(colors.border_variant)
            .bg(colors.editor_background)
            .overflow_hidden()
            .map(|this| match &diff {
                Some(diff) => this.py_1().font_buffer(cx).text_xs().children(
                    diff.iter().take(MAX_DIFF_LINES).map(|(kind, line)| {
                        let (sign, background) = match kind {
                            DiffLine::Context => (" ", None),
                            DiffLine::Removed => ("-", Some(status_colors.deleted_background)),
                            DiffLine::Added => ("+", Some(status_colors.created_background)),
                        };
                        h_flex()
                            .px_2()
                            .gap_2()
                            .when_some(background, |this, background| this.bg(background))
                            .child(div().flex_none().text_color(colors.text_muted).child(sign))
                            .child(div().min_w_0().child(line.to_string()))
                    }),
                ),
                None => this.child(block("IN", input_text)),
            })
            .when_some(output_text, |this, (output, more)| {
                this.child(div().h_px().bg(colors.border_variant))
                    .child(block("OUT", output))
                    .when(more, |this| {
                        this.child(
                            h_flex().px_2().pb_1().justify_end().child(
                                Button::new(("full-output", index), "Show more")
                                    .label_size(LabelSize::XSmall)
                                    .color(Color::Muted)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.full_output.insert(index);
                                        cx.notify();
                                    })),
                            ),
                        )
                    })
            });
        let content = v_flex()
            .w_full()
            .min_w_0()
            .child(header)
            .when(expanded, |this| this.child(body));
        timeline_row(
            index,
            dot,
            running,
            content.into_any_element(),
            None,
            self.rail(index, cx),
        )
    }

    fn render_permission(
        &self,
        index: usize,
        request: &PermissionRequest,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors();
        let tool = &request.request;
        let summary = ToolCall {
            id: String::new(),
            name: tool.tool_name.clone(),
            input: tool.input.clone(),
            partial_input: String::new(),
            result: None,
            status: ToolStatus::Running,
            timestamp: request.timestamp,
            parent_tool_use_id: None,
        };
        if request.status != PermissionStatus::Pending {
            let (label, color) = match request.status {
                PermissionStatus::Allowed => ("Allowed", Color::Success),
                PermissionStatus::Denied => ("Denied", Color::Error),
                _ => ("Cancelled", Color::Muted),
            };
            return timeline_row(
                index,
                color,
                false,
                h_flex()
                    .gap_1()
                    .child(
                        Label::new(format!("{label}: {}", display_tool_name(&tool.tool_name)))
                            .size(LabelSize::Small)
                            .color(color),
                    )
                    .child(
                        Label::new(summary.summary())
                            .size(LabelSize::Small)
                            .color(Color::Muted)
                            .truncate(),
                    )
                    .into_any_element(),
                None,
                self.rail(index, cx),
            );
        }
        let active = self.card_request.as_deref() == Some(request.request_id.as_str());
        if let Some(form) = self
            .question_form
            .as_ref()
            .filter(|form| active && form.request.request_id == request.request_id)
        {
            return self.render_question_card(index, form, cx);
        }
        let question = tool.title.clone().unwrap_or_else(|| {
            format!(
                "Allow Claude to use {}?",
                display_tool_name(&tool.tool_name)
            )
        });
        let options = Self::permission_options(request);
        let selected = active.then_some(self.card_selection);
        v_flex()
            .id(("permission", index))
            .w_full()
            .gap_1p5()
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(colors.border_focused)
            .bg(colors.elevated_surface_background)
            .key_context("ClaudeCodePermission")
            .when(active, |this| {
                this.track_focus(&self.card_focus)
                    .on_key_down(cx.listener(Self::card_key_down))
            })
            .child(
                Label::new(question)
                    .size(LabelSize::Small)
                    .weight(gpui::FontWeight::SEMIBOLD),
            )
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(colors.editor_background)
                    .font_buffer(cx)
                    .text_xs()
                    .child(truncate_lines(&tool_input_text(&summary), 12)),
            )
            .when_some(tool.description.clone(), |this, description| {
                this.child(
                    Label::new(description)
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                )
            })
            .child(
                v_flex()
                    .gap_0p5()
                    .children(options.into_iter().enumerate().map(|(option, (label, _))| {
                        option_row(
                            ("permission-option", option),
                            option + 1,
                            None,
                            label,
                            None,
                            selected == Some(option),
                            cx,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.choose_permission_option(option, window, cx)
                            },
                        ))
                    })),
            )
            .when(active, |this| {
                this.child(
                    div()
                        .ml(px(26.))
                        .px_1()
                        .rounded_sm()
                        .border_1()
                        .border_color(colors.border_variant)
                        .bg(colors.editor_background)
                        .key_context("ClaudeCodeDenyInput")
                        .child(self.deny_input.clone()),
                )
                .child(
                    Label::new("↑↓ to select · Enter to choose · Esc to cancel")
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                )
            })
            .into_any_element()
    }

    /// AskUserQuestion: each question with its options, an "Other" field,
    /// and one Submit for all of them.
    fn render_question_card(
        &self,
        index: usize,
        form: &QuestionForm,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors();
        let can_submit = form.answers(cx).is_some();
        let questions =
            form.questions
                .iter()
                .enumerate()
                .map(|(question_index, question)| {
                    let first_row = form.first_row_of(question_index);
                    let chosen = form.chosen.get(question_index);
                    let other_row = first_row + question.options.len();
                    let other_number = question.options.len() + 1;
                    v_flex()
                        .gap_0p5()
                        .child(
                            h_flex()
                                .gap_1p5()
                                .when(!question.header.is_empty(), |this| {
                                    this.child(
                                        div()
                                            .px_1()
                                            .rounded_sm()
                                            .bg(colors.element_background)
                                            .child(
                                                Label::new(question.header.clone())
                                                    .size(LabelSize::XSmall)
                                                    .color(Color::Muted),
                                            ),
                                    )
                                })
                                .child(
                                    Label::new(question.question.clone())
                                        .size(LabelSize::Small)
                                        .weight(gpui::FontWeight::SEMIBOLD),
                                ),
                        )
                        .when(question.multi_select, |this| {
                            this.child(
                                Label::new("Choose any that apply")
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            )
                        })
                        .children(question.options.iter().enumerate().map(
                            |(option_index, option)| {
                                let row = first_row + option_index;
                                option_row(
                                    ("question-option", row),
                                    option_index + 1,
                                    Some(
                                        chosen.is_some_and(|chosen| chosen.contains(&option_index)),
                                    ),
                                    option.label.clone(),
                                    Some(option.description.clone())
                                        .filter(|text| !text.is_empty()),
                                    form.cursor == row,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        this.choose_question_option(
                                            question_index,
                                            option_index,
                                            window,
                                            cx,
                                        )
                                    },
                                ))
                            },
                        ))
                        .children(form.other.get(question_index).map(|editor| {
                            let other_chosen = chosen
                                .is_some_and(|chosen| chosen.contains(&question.options.len()));
                            h_flex()
                                .id(("question-other", other_row))
                                .gap_2()
                                .px_2()
                                .py_0p5()
                                .rounded_sm()
                                .when(form.cursor == other_row, |this| {
                                    this.bg(colors.element_selected)
                                })
                                .child(option_marker(other_number, Some(other_chosen)))
                                .child(
                                    div()
                                        .flex_1()
                                        .px_1()
                                        .rounded_sm()
                                        .border_1()
                                        .border_color(colors.border_variant)
                                        .bg(colors.editor_background)
                                        .key_context("ClaudeCodeQuestionOther")
                                        .child(editor.clone()),
                                )
                        }))
                });
        v_flex()
            .id(("question", index))
            .w_full()
            .gap_2()
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(colors.border_focused)
            .bg(colors.elevated_surface_background)
            .key_context("ClaudeCodeQuestion")
            .track_focus(&self.card_focus)
            .on_key_down(cx.listener(Self::card_key_down))
            .children(questions)
            .child(
                h_flex()
                    .justify_between()
                    .gap_1()
                    .child(
                        Label::new("↑↓ to select · Enter to choose · Esc to dismiss")
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new(("dismiss-question", index), "Dismiss")
                                    .style(ButtonStyle::Subtle)
                                    .label_size(LabelSize::Small)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.dismiss_question(&DismissQuestion, window, cx)
                                    })),
                            )
                            .child(
                                Button::new(("submit-answers", index), "Submit")
                                    .style(ButtonStyle::Filled)
                                    .label_size(LabelSize::Small)
                                    .disabled(!can_submit)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.submit_answers(&SubmitAnswers, window, cx)
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        h_flex()
            .h(px(32.))
            .px_2()
            .gap_1()
            .flex_none()
            .justify_between()
            .border_b_1()
            .border_color(colors.border)
            .child(Label::new(self.title(cx)).size(LabelSize::Small).truncate())
            .child(
                h_flex()
                    .gap_0p5()
                    .child(
                        IconButton::new("focus-view", IconName::Eye)
                            .icon_size(HEADER_ICON_SIZE)
                            .size(ButtonSize::Medium)
                            .toggle_state(self.focus_view)
                            .tooltip(move |_, cx| {
                                Tooltip::for_action("Toggle Focus View", &ToggleFocusView, cx)
                            })
                            .on_click(|_, window, cx| {
                                window.dispatch_action(ToggleFocusView.boxed_clone(), cx)
                            }),
                    )
                    .child(
                        IconButton::new("sessions", IconName::HistoryRerun)
                            .icon_size(HEADER_ICON_SIZE)
                            .size(ButtonSize::Medium)
                            .tooltip(move |_, cx| {
                                Tooltip::for_action("Past Conversations", &crate::ShowSessions, cx)
                            })
                            .on_click(|_, window, cx| {
                                window.dispatch_action(crate::ShowSessions.boxed_clone(), cx)
                            }),
                    )
                    .child(
                        IconButton::new("new-conversation", IconName::Plus)
                            .icon_size(HEADER_ICON_SIZE)
                            .size(ButtonSize::Medium)
                            .tooltip(move |_, cx| {
                                Tooltip::for_action("New Conversation", &NewConversation, cx)
                            })
                            .on_click(|_, window, cx| {
                                window.dispatch_action(NewConversation.boxed_clone(), cx)
                            }),
                    ),
            )
    }

    fn render_spinner(&self, cx: &App) -> Option<AnyElement> {
        let conversation = &self.session.read(cx).conversation;
        if conversation.turn == TurnState::Idle {
            return None;
        }
        let label = if conversation.turn == TurnState::RequiresAction {
            "Waiting for your answer…".to_string()
        } else if let Some(tool) = &conversation.running_tool {
            format!("Running {}…", display_tool_name(tool))
        } else {
            let verbs = &self.spinner_verbs;
            let verb = verbs
                .get(self.spinner_index % verbs.len().max(1))
                .cloned()
                .unwrap_or_else(|| "Working".to_string());
            format!("{verb}…")
        };
        let elapsed = conversation
            .turn_started_at
            .map(|started| (Utc::now() - started).num_seconds().max(0));
        Some(
            h_flex()
                .px_3()
                .gap_1p5()
                .child(
                    Icon::new(IconName::Sparkle)
                        .size(IconSize::XSmall)
                        .color(Color::Accent),
                )
                .child(
                    Label::new(label)
                        .size(LabelSize::Small)
                        .color(Color::Accent),
                )
                .when_some(elapsed, |this, elapsed| {
                    this.child(
                        Label::new(format!("{elapsed}s · Esc to interrupt"))
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
                })
                .into_any_element(),
        )
    }

    fn render_chips(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let context = self.effective_context(cx);
        if context.is_none() && self.attachments.is_empty() {
            return None;
        }
        let mut chips: Vec<AnyElement> = Vec::new();
        if let Some(context) = context {
            let name = file_name(&context.path);
            let label = match &context.selection {
                Some((start, end, _)) => format!("{name}#{}-{}", start + 1, end + 1),
                None => name,
            };
            chips.push(
                chip(
                    "context-chip",
                    IconName::File,
                    label,
                    cx.listener(|this, _, _, cx| {
                        this.context_dismissed = true;
                        cx.notify();
                    }),
                    cx,
                )
                .into_any_element(),
            );
        }
        for (index, attachment) in self.attachments.iter().enumerate() {
            let (icon, label) = match attachment {
                Attachment::Mention(path) => (IconName::AtSign, file_name(path)),
                Attachment::Selection { path, rows } => (
                    IconName::AtSign,
                    format!("{}#{}-{}", file_name(path), rows.0 + 1, rows.1 + 1),
                ),
                Attachment::Image { name, .. } => (IconName::Image, name.to_string()),
            };
            chips.push(
                chip(
                    ("attachment", index),
                    icon,
                    label,
                    cx.listener(move |this, _, _, cx| {
                        if index < this.attachments.len() {
                            this.attachments.remove(index);
                        }
                        cx.notify();
                    }),
                    cx,
                )
                .into_any_element(),
            );
        }
        Some(
            h_flex()
                .flex_wrap()
                .gap_1()
                .px_2()
                .pt_1p5()
                .children(chips)
                .into_any_element(),
        )
    }

    fn render_input(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let settings = ClaudeCodeSettings::get_global(cx);
        let running = self.is_running(cx);
        let mut key_context = KeyContext::default();
        key_context.add("ClaudeCodeInput");
        if settings.use_ctrl_enter_to_send {
            key_context.add("use_ctrl_enter_to_send");
        }
        if self.shown_suggestion(cx).is_some() {
            key_context.add("has_suggestion");
        }
        if !self.visible_commands(cx).is_empty() {
            key_context.add("showing_commands");
        }
        let send_button = if running && self.input.read(cx).text(cx).trim().is_empty() {
            IconButton::new("stop", IconName::Stop)
                .shape(IconButtonShape::Square)
                .icon_size(IconSize::Medium)
                .size(ButtonSize::Medium)
                .style(ButtonStyle::Filled)
                .icon_color(Color::Error)
                .tooltip(move |_, cx| Tooltip::for_action("Stop", &Interrupt, cx))
                .on_click(|_, window, cx| window.dispatch_action(Interrupt.boxed_clone(), cx))
        } else {
            IconButton::new("send", IconName::ArrowUp)
                .shape(IconButtonShape::Square)
                .icon_size(SEND_ICON_SIZE)
                .size(ButtonSize::Medium)
                .style(ButtonStyle::Filled)
                .icon_color(Color::Accent)
                .tooltip(move |_, cx| Tooltip::for_action("Send", &Send, cx))
                .on_click(|_, window, cx| window.dispatch_action(Send.boxed_clone(), cx))
        };
        v_flex()
            .m_2()
            .flex_none()
            .rounded_lg()
            .border_1()
            .border_color(if self.input.focus_handle(cx).is_focused(window) {
                colors.border_focused
            } else {
                colors.border
            })
            .bg(colors.editor_background)
            .when_some(self.banner.clone(), |this, banner| {
                this.child(
                    h_flex()
                        .px_2()
                        .py_1()
                        .justify_between()
                        .border_b_1()
                        .border_color(colors.border_variant)
                        .child(
                            Label::new(banner)
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        )
                        .child(
                            IconButton::new("dismiss-banner", IconName::Close)
                                .icon_size(IconSize::XSmall)
                                .on_click(cx.listener(|this, _, _, cx| this.set_banner(None, cx))),
                        ),
                )
            })
            .when(
                self.session.read(cx).conversation.needs_login
                    && !ClaudeCodeSettings::get_global(cx).disable_login_prompt,
                |this| {
                    this.child(
                        h_flex()
                            .px_2()
                            .py_1()
                            .gap_2()
                            .justify_between()
                            .border_b_1()
                            .border_color(colors.border_variant)
                            .child(
                                Label::new("Claude Code is not signed in.")
                                    .size(LabelSize::XSmall)
                                    .color(Color::Warning),
                            )
                            .child(
                                h_flex()
                                    .gap_1()
                                    .child(
                                        Button::new("log-in", "Log in")
                                            .label_size(LabelSize::XSmall)
                                            .style(ButtonStyle::Filled)
                                            .on_click(|_, window, cx| {
                                                window
                                                    .dispatch_action(crate::Login.boxed_clone(), cx)
                                            }),
                                    )
                                    .child(
                                        IconButton::new("dismiss-login", IconName::Close)
                                            .icon_size(IconSize::XSmall)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.session.update(cx, |session, _| {
                                                    session.conversation.needs_login = false
                                                });
                                                cx.notify();
                                            })),
                                    ),
                            ),
                    )
                },
            )
            .children(self.render_chips(cx))
            .children(self.render_command_list(cx))
            .child(
                div()
                    .key_context(key_context)
                    .px_2()
                    .pt_1p5()
                    .child(self.input.clone()),
            )
            .child(
                h_flex()
                    .px_1()
                    .pb_1()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap_0p5()
                            .child(self.render_attach_menu(cx))
                            .child(self.render_command_menu(cx))
                            .children(self.render_usage(cx))
                            .child(self.render_model_menu(cx)),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(self.render_mode_menu(cx))
                            .child(send_button),
                    ),
            )
    }

    fn render_attach_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.weak_entity();
        PopoverMenu::new("attach-menu")
            .trigger_with_tooltip(
                IconButton::new("attach", IconName::Plus)
                    .icon_size(IconSize::Medium)
                    .size(ButtonSize::Medium),
                Tooltip::text("Add context"),
            )
            .anchor(gpui::Anchor::BottomLeft)
            .menu(move |window, cx| {
                let this = this.clone();
                Some(ContextMenu::build(window, cx, move |menu, _, _| {
                    menu.entry("Mention file from this project…", None, {
                        let this = this.clone();
                        move |window, cx| {
                            this.update(cx, |this, cx| this.open_file_picker(window, cx))
                                .ok();
                        }
                    })
                    .entry("Attach file…", None, {
                        move |window, cx| {
                            this.update(cx, |this, cx| this.attach_files_from_disk(window, cx))
                                .ok();
                        }
                    })
                    .action(
                        "Insert @-Mention of Selection",
                        crate::InsertAtMention.boxed_clone(),
                    )
                }))
            })
    }

    fn render_command_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        IconButton::new("commands", IconName::Slash)
            .icon_size(IconSize::Medium)
            .size(ButtonSize::Medium)
            .tooltip(Tooltip::text("Commands"))
            .on_click(cx.listener(|this, _, window, cx| {
                this.commands_dismissed = false;
                this.command_selection = 0;
                this.input.update(cx, |input, cx| {
                    input.set_text("/", window, cx);
                    input.move_to_end(&editor::actions::MoveToEnd, window, cx);
                });
                window.focus(&this.input.focus_handle(cx), cx);
                cx.notify();
            }))
    }

    /// The rows of the "/" list for what is typed, empty when it is closed.
    fn visible_commands(&self, cx: &App) -> Vec<CommandEntry> {
        if self.commands_dismissed {
            return Vec::new();
        }
        if let Some(mention) = &self.mention {
            return mention.entries.clone();
        }
        let text = self.input.read(cx).text(cx);
        let Some(query) = command_query(&text) else {
            return Vec::new();
        };
        let slash_commands = self
            .session
            .read(cx)
            .initialize
            .as_ref()
            .map(|init| {
                init.commands
                    .iter()
                    .map(|command| (command.name.clone(), command.description.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        matching_commands(query, &slash_commands)
    }

    fn render_command_list(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let entries = self.visible_commands(cx);
        let last = entries.len().checked_sub(1)?;
        let selected = self.command_selection.min(last);
        let colors = cx.theme().colors();
        Some(
            v_flex()
                .id("command-list")
                .mx_2()
                .mt_1p5()
                .py_0p5()
                .max_h(px(COMMAND_LIST_MAX_HEIGHT))
                .overflow_y_scroll()
                .track_scroll(&self.command_scroll)
                .rounded_sm()
                .border_1()
                .border_color(colors.border_variant)
                .bg(colors.elevated_surface_background)
                .children(entries.into_iter().enumerate().map(|(index, entry)| {
                    let row = h_flex()
                        .id(("command", index))
                        .px_2()
                        .py_0p5()
                        .cursor_pointer()
                        .when(index == selected, |this| this.bg(colors.element_selected))
                        .hover(|this| this.bg(colors.element_hover));
                    let row = match &entry.kind {
                        CommandKind::Mention {
                            is_dir, positions, ..
                        } => row
                            .gap_1p5()
                            .child(
                                Icon::new(if *is_dir {
                                    IconName::Folder
                                } else {
                                    IconName::File
                                })
                                .size(IconSize::Small)
                                .color(Color::Muted),
                            )
                            .child(
                                HighlightedLabel::new(entry.label.clone(), positions.clone())
                                    .size(LabelSize::Small)
                                    .truncate(),
                            ),
                        CommandKind::Slash(_) | CommandKind::Action(_) => row
                            .gap_3()
                            .child(Label::new(entry.label.clone()).size(LabelSize::Small))
                            .child(
                                Label::new(entry.description.clone())
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted)
                                    .truncate(),
                            ),
                    };
                    row.on_click(cx.listener(move |this, _, window, cx| {
                        this.accept_command(entry.clone(), window, cx);
                    }))
                }))
                .into_any_element(),
        )
    }

    fn select_next_command(
        &mut self,
        _: &SelectNextCommand,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_command_selection(1, cx);
    }

    fn select_previous_command(
        &mut self,
        _: &SelectPreviousCommand,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_command_selection(-1, cx);
    }

    fn move_command_selection(&mut self, step: isize, cx: &mut Context<Self>) {
        let count = self.visible_commands(cx).len();
        if count == 0 {
            cx.propagate();
            return;
        }
        let current = self.command_selection.min(count - 1);
        self.command_selection = (current as isize + step).rem_euclid(count as isize) as usize;
        self.command_scroll.scroll_to_item(self.command_selection);
        cx.notify();
    }

    fn confirm_command(&mut self, _: &ConfirmCommand, window: &mut Window, cx: &mut Context<Self>) {
        let entries = self.visible_commands(cx);
        let Some(entry) = entries
            .get(self.command_selection.min(entries.len().saturating_sub(1)))
            .cloned()
        else {
            cx.propagate();
            return;
        };
        self.accept_command(entry, window, cx);
    }

    fn dismiss_commands(&mut self, _: &DismissCommands, _: &mut Window, cx: &mut Context<Self>) {
        self.commands_dismissed = true;
        cx.notify();
    }

    fn accept_command(&mut self, entry: CommandEntry, window: &mut Window, cx: &mut Context<Self>) {
        self.command_selection = 0;
        match entry.kind {
            CommandKind::Slash(name) => {
                self.input.update(cx, |input, cx| {
                    input.set_text(format!("/{name} "), window, cx);
                    input.move_to_end(&editor::actions::MoveToEnd, window, cx);
                });
                window.focus(&self.input.focus_handle(cx), cx);
            }
            CommandKind::Action(action) => {
                self.input
                    .update(cx, |input, cx| input.set_text("", window, cx));
                window.dispatch_action(action(), cx);
            }
            CommandKind::Mention { path, is_dir, .. } => {
                if let Some(mention) = self.mention.take() {
                    self.input.update(cx, |input, cx| {
                        let text = input.text(cx);
                        let insertion = mention_insertion(&text, &mention.range, &path, is_dir);
                        input.change_selections(SelectionEffects::default(), window, cx, |s| {
                            s.select_ranges([MultiBufferOffset(mention.range.start)
                                ..MultiBufferOffset(mention.range.end)])
                        });
                        input.insert(&insertion, window, cx);
                    });
                }
                window.focus(&self.input.focus_handle(cx), cx);
            }
        }
        cx.notify();
    }

    /// Follows the "@" mention under the cursor, matching the project's files
    /// and folders against what follows the "@".
    fn update_mention(&mut self, cx: &mut Context<Self>) {
        let (text, cursor) = self.input.update(cx, |input, cx| {
            let snapshot = input.display_snapshot(cx);
            let cursor = input
                .selections
                .newest::<MultiBufferOffset>(&snapshot)
                .head();
            (input.text(cx), cursor.0)
        });
        let Some((range, query)) = mention_query(&text, cursor) else {
            if self.mention.take().is_some() {
                cx.notify();
            }
            self.mention_candidates = None;
            self._mention_matches = None;
            if command_query(&text).is_none() {
                self.commands_dismissed = false;
            }
            return;
        };
        if self
            .mention
            .as_ref()
            .is_none_or(|mention| mention.range.start != range.start)
        {
            // Escape closes the list for one mention, not for the next "@".
            self.commands_dismissed = false;
        }
        if let Some(mention) = self.mention.as_mut()
            && mention.query == query
        {
            mention.range = range;
            return;
        }
        let entries = self
            .mention
            .take()
            .map(|mention| mention.entries)
            .unwrap_or_default();
        self.mention = Some(MentionState {
            range,
            query: query.to_string(),
            entries,
        });
        let candidates = match &self.mention_candidates {
            Some(candidates) => candidates.clone(),
            None => {
                let respect_git_ignore = ClaudeCodeSettings::get_global(cx).respect_git_ignore;
                let entries = self
                    .workspace
                    .upgrade()
                    .map(|workspace| {
                        let project = workspace_project(workspace.read(cx));
                        project_entries(project.read(cx), respect_git_ignore, cx)
                    })
                    .unwrap_or_default();
                let candidates = Arc::new(entries);
                self.mention_candidates = Some(candidates.clone());
                candidates
            }
        };
        let query = query.to_string();
        let executor = cx.background_executor().clone();
        self._mention_matches = Some(cx.spawn(async move |this, cx| {
            let matches = match_entries(&candidates, &query, MENTION_MAX_RESULTS, executor).await;
            this.update(cx, |this, cx| {
                let entries = matches
                    .into_iter()
                    .filter_map(|found| {
                        let entry = candidates.get(found.candidate_id)?;
                        Some(CommandEntry {
                            label: entry.display.clone().into(),
                            description: SharedString::default(),
                            kind: CommandKind::Mention {
                                path: this.relative_path(&entry.abs_path, cx),
                                is_dir: entry.is_dir,
                                positions: found.positions,
                            },
                        })
                    })
                    .collect();
                if let Some(mention) = this.mention.as_mut()
                    && mention.query == query
                {
                    mention.entries = entries;
                    this.command_selection = 0;
                    this.command_scroll.scroll_to_item(0);
                    cx.notify();
                }
            })
            .ok();
        }));
        cx.notify();
    }

    fn render_usage(&self, cx: &App) -> Option<AnyElement> {
        let conversation = &self.session.read(cx).conversation;
        let mut parts = Vec::new();
        if let Some(resets_at) = conversation
            .rate_limit
            .as_ref()
            .and_then(|limit| limit.resets_at)
        {
            let remaining = resets_at - Utc::now().timestamp();
            if remaining > 0 {
                parts.push(format_duration(remaining));
            }
        }
        if let Some(usage) = &conversation.last_usage {
            let tokens = usage.context_tokens() + usage.output_tokens;
            parts.push(format!("{}k", tokens.div_ceil(1000)));
        }
        if parts.is_empty() {
            return None;
        }
        let tooltip = format!(
            "Context: {} tokens · Cost so far: ${:.2}",
            conversation
                .last_usage
                .as_ref()
                .map(|usage| usage.context_tokens())
                .unwrap_or_default(),
            conversation.total_cost_usd
        );
        Some(
            h_flex()
                .id("usage")
                .px_1()
                .gap_0p5()
                .child(
                    Icon::new(IconName::Clock)
                        .size(IconSize::XSmall)
                        .color(Color::Muted),
                )
                .child(
                    Label::new(parts.join(" · "))
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                )
                .tooltip(Tooltip::text(tooltip))
                .into_any_element(),
        )
    }

    fn model_label(&self, cx: &App) -> String {
        let session = self.session.read(cx);
        let models = session
            .initialize
            .as_ref()
            .map(|init| init.models.as_slice())
            .unwrap_or_default();
        let selected = session.selected_model();
        let resolved = session.conversation.model.as_deref();
        models
            .iter()
            .find(|model| Some(model.value.as_str()) == selected)
            .or_else(|| {
                models.iter().find(|model| {
                    selected.is_none()
                        && model.resolved_model.as_deref() == resolved
                        && model.value != "default"
                })
            })
            .map(|model| model.display_name.clone())
            .or_else(|| resolved.map(str::to_string))
            .unwrap_or_else(|| "Default".to_string())
    }

    fn render_model_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.weak_entity();
        let models = self
            .session
            .read(cx)
            .initialize
            .as_ref()
            .map(|init| init.models.clone())
            .unwrap_or_default();
        let selected = self.session.read(cx).selected_model().map(str::to_string);
        let thinking = self.thinking_enabled;
        PopoverMenu::new("model-menu")
            .trigger_with_tooltip(
                Button::new("model", self.model_label(cx))
                    .style(ButtonStyle::Filled)
                    .label_size(LabelSize::XSmall)
                    .color(Color::Muted),
                Tooltip::text("Switch model"),
            )
            .anchor(gpui::Anchor::BottomLeft)
            .menu(move |window, cx| {
                let this = this.clone();
                let models = models.clone();
                let selected = selected.clone();
                Some(ContextMenu::build(window, cx, move |mut menu, _, _| {
                    for model in models {
                        let is_selected = selected.as_deref() == Some(model.value.as_str())
                            || (selected.is_none() && model.value == "default");
                        let this = this.clone();
                        let value = model.value.clone();
                        let mut entry = ContextMenuEntry::new(model.display_name.clone())
                            .toggleable(IconPosition::End, is_selected);
                        if !model.description.is_empty() {
                            let description = model.description.clone();
                            entry = entry
                                .documentation_aside(ui::DocumentationSide::Right, move |_| {
                                    Label::new(description.clone()).into_any_element()
                                });
                        }
                        menu.push_item(entry.handler(move |_, cx| {
                            this.update(cx, |this, cx| {
                                let model = (value != "default").then(|| value.clone());
                                this.session
                                    .update(cx, |session, cx| session.set_model(model, cx));
                            })
                            .ok();
                        }));
                    }
                    menu.separator().toggleable_entry(
                        "Thinking",
                        thinking,
                        IconPosition::End,
                        Some(ToggleThinking.boxed_clone()),
                        |window, cx| window.dispatch_action(ToggleThinking.boxed_clone(), cx),
                    )
                }))
            })
    }

    fn render_mode_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.weak_entity();
        let modes = self.available_modes(cx);
        let current = self
            .session
            .read(cx)
            .conversation
            .permission_mode
            .unwrap_or(PermissionMode::Default);
        PopoverMenu::new("mode-menu")
            .trigger_with_tooltip(
                Button::new("mode", current.label())
                    .label_size(LabelSize::XSmall)
                    .color(if current == PermissionMode::BypassPermissions {
                        Color::Warning
                    } else {
                        Color::Muted
                    })
                    .start_icon(
                        Icon::new(mode_icon(current))
                            .size(IconSize::XSmall)
                            .color(Color::Muted),
                    ),
                Tooltip::for_action_title("Change mode", &CycleMode),
            )
            .anchor(gpui::Anchor::BottomRight)
            .menu(move |window, cx| {
                let this = this.clone();
                let modes = modes.clone();
                Some(ContextMenu::build(window, cx, move |mut menu, _, _| {
                    for mode in modes {
                        let this = this.clone();
                        menu.push_item(
                            ContextMenuEntry::new(mode.label())
                                .icon(mode_icon(mode))
                                .toggleable(IconPosition::End, mode == current)
                                .handler(move |_, cx| {
                                    this.update(cx, |this, cx| {
                                        this.session.update(cx, |session, cx| {
                                            session.set_permission_mode(mode, cx)
                                        });
                                    })
                                    .ok();
                                }),
                        );
                    }
                    menu
                }))
            })
    }

    fn open_file_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let view = cx.weak_entity();
        let respect_git_ignore = ClaudeCodeSettings::get_global(cx).respect_git_ignore;
        workspace.update(cx, |workspace, cx| {
            let project = workspace_project(workspace);
            workspace.toggle_modal(window, cx, |window, cx| {
                crate::file_picker::new_file_picker(view, project, respect_git_ignore, window, cx)
            });
        });
    }

    fn attach_files_from_disk(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some("Attach".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            this.update(cx, |this, cx| {
                for path in paths {
                    this.add_attachment(Attachment::Mention(path), cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn render_jump_to_bottom(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.shows_jump_to_bottom() {
            return None;
        }
        let colors = cx.theme().colors();
        Some(
            h_flex()
                .absolute()
                .bottom_2()
                .left_0()
                .right_0()
                .justify_center()
                .child(
                    div()
                        .rounded_full()
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.elevated_surface_background)
                        .shadow_md()
                        .overflow_hidden()
                        .debug_selector(|| "jump-to-bottom".into())
                        .child(
                            IconButton::new("jump-to-bottom", IconName::ArrowDown)
                                .shape(IconButtonShape::Square)
                                .icon_size(IconSize::Small)
                                .tooltip(Tooltip::text("Scroll to latest messages"))
                                .on_click(cx.listener(|this, _, _, cx| this.jump_to_bottom(cx))),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Sparse, as in the extension: a small logo and one line in the middle,
    /// the message box below.
    fn render_empty_state(&self, cx: &App) -> AnyElement {
        let session = self.session.read(cx);
        let settings = ClaudeCodeSettings::get_global(cx);
        let mut content = v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .p_4()
            .child(
                Icon::new(IconName::Sparkle)
                    .size(IconSize::Medium)
                    .color(Color::Accent),
            )
            .child(
                Label::new("Ask Claude to explain, edit or build code in this project.")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            );
        if let ProcessStatus::Exited(message) = &session.status {
            content = content.child(
                Label::new(message.clone())
                    .size(LabelSize::Small)
                    .color(Color::Error),
            );
        }
        if !settings.hide_onboarding {
            content = content.child(crate::onboarding::render_checklist(cx));
        }
        content.into_any_element()
    }
}

/// Claude's replies in the size of the rest of the panel rather than the
/// larger agent font, with code a step smaller, as in the extension.
fn message_markdown_style(window: &Window, cx: &App) -> MarkdownStyle {
    let mut style = MarkdownStyle::themed(MarkdownFont::Agent, window, cx);
    let text_size = TextSize::Default.rems(cx);
    let code_size = rems_from_px(13_f32);
    style.base_text_style.font_size = text_size.into();
    // Paragraphs take their size from the container, not the base style.
    style.container_style.text.font_size = Some(text_size.into());
    style.base_text_style.line_height = gpui::relative(1.5);
    style.code_block.text.font_size = Some(code_size.into());
    style.inline_code.font_size = Some(code_size.into());
    style.paragraph_line_height = gpui::relative(1.5);
    style
}

/// Tool calls' input and output: the buffer font, small, as plain text.
fn tool_block_markdown_style(window: &Window, cx: &App) -> MarkdownStyle {
    let mut style = MarkdownStyle::themed(MarkdownFont::Editor, window, cx);
    if let Some(family) = style.inline_code.font_family.clone() {
        style.base_text_style.font_family = family;
    }
    let size = rems_from_px(12_f32);
    style.base_text_style.font_size = size.into();
    style.container_style.text.font_size = Some(size.into());
    style
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum PermissionChoice {
    Allow,
    AllowAlways,
    Deny,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum PlainPart {
    Message,
    ToolInput,
    ToolOutput,
}

fn workspace_project(workspace: &Workspace) -> Entity<Project> {
    workspace.project().clone()
}

#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    Allow,
    AllowWithInput(Value),
    AllowAlways,
    /// No, with what the person typed, if anything, for Claude to do instead.
    Deny(String),
    /// No, telling Claude exactly this, and letting the turn go on.
    Dismiss(String),
}

impl Focusable for ClaudeCodeView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ClaudeCodeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.update_follow_tail();
        self.sync_card(window, cx);
        let entries = self.render_entries(window, cx);
        let empty = entries.is_empty();
        let jump_to_bottom = self.render_jump_to_bottom(cx);
        let colors = cx.theme().colors();
        let mut key_context = KeyContext::default();
        key_context.add("ClaudeCode");
        if ClaudeCodeSettings::get_global(cx).enable_new_conversation_shortcut {
            key_context.add("new_conversation_shortcut");
        }
        v_flex()
            .size_full()
            .key_context(key_context)
            .track_focus(&self.focus_handle)
            .bg(colors.panel_background)
            .on_action(cx.listener(Self::send))
            .on_action(cx.listener(Self::interrupt))
            .on_action(cx.listener(Self::toggle_focus_view))
            .on_action(cx.listener(Self::toggle_thinking))
            .on_action(cx.listener(Self::focus_input))
            .on_action(cx.listener(Self::blur_input))
            .on_action(cx.listener(Self::focus_last_message))
            .on_action(cx.listener(Self::cycle_mode))
            .on_action(cx.listener(Self::select_next_command))
            .on_action(cx.listener(Self::select_previous_command))
            .on_action(cx.listener(Self::confirm_command))
            .on_action(cx.listener(Self::dismiss_commands))
            .on_action(cx.listener(Self::accept_suggestion))
            .on_action(cx.listener(Self::allow_permission))
            .on_action(cx.listener(Self::allow_permission_always))
            .on_action(cx.listener(Self::deny_permission))
            .on_action(cx.listener(Self::select_next_option))
            .on_action(cx.listener(Self::select_previous_option))
            .on_action(cx.listener(Self::confirm_option))
            .on_action(cx.listener(Self::submit_answers))
            .on_action(cx.listener(Self::dismiss_question))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::new_conversation_requested))
            .child(self.render_header(cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("claude-messages")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll_handle)
                            .on_scroll(cx.listener(|this, _, _, cx| {
                                this.scrolled_by_user = true;
                                cx.notify();
                            }))
                            .child(if empty {
                                self.render_empty_state(cx)
                            } else {
                                v_flex().p_2().gap_2().children(entries).into_any_element()
                            }),
                    )
                    .children(jump_to_bottom),
            )
            .children(self.render_spinner(cx))
            .child(self.render_input(window, cx))
    }
}

/// One step of the reply: a dot, the content, and, when another step follows,
/// a rail from this dot down to the next one.
fn timeline_row(
    index: usize,
    dot: Color,
    animated: bool,
    content: AnyElement,
    timestamp: Option<String>,
    rail: Option<gpui::Hsla>,
) -> AnyElement {
    let dot_element = Icon::new(if animated {
        IconName::LoadCircle
    } else {
        IconName::Circle
    })
    .size(IconSize::XSmall)
    .color(dot);
    let dot_element = if animated {
        dot_element.with_rotate_animation(2).into_any_element()
    } else {
        dot_element.into_any_element()
    };
    h_flex()
        .id(("timeline", index))
        .w_full()
        .items_start()
        .gap_2()
        .child(
            div()
                .relative()
                .self_stretch()
                .flex_none()
                .pt_0p5()
                .child(dot_element)
                .when_some(rail, |this, rail| {
                    // Reaches past the row into the list's gap, to the next dot.
                    this.child(
                        div()
                            .absolute()
                            .top(px(16.))
                            .bottom(px(-6.))
                            .left(px(5.5))
                            .w_px()
                            .bg(rail),
                    )
                }),
        )
        .child(div().flex_1().min_w_0().child(content))
        .when_some(timestamp, |this, timestamp| {
            this.child(
                div().flex_none().child(
                    Label::new(timestamp)
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                ),
            )
        })
        .into_any_element()
}

/// A numbered option of the permission or question card. `chosen` is None
/// where options are picked once rather than checked.
fn option_row(
    id: impl Into<ElementId>,
    number: usize,
    chosen: Option<bool>,
    label: String,
    description: Option<String>,
    highlighted: bool,
    cx: &App,
) -> gpui::Stateful<gpui::Div> {
    let colors = cx.theme().colors();
    h_flex()
        .id(id)
        .items_start()
        .gap_2()
        .px_2()
        .py_0p5()
        .rounded_sm()
        .cursor_pointer()
        .when(highlighted, |this| this.bg(colors.element_selected))
        .hover(|this| this.bg(colors.element_hover))
        .child(option_marker(number, chosen))
        .child(
            v_flex()
                .min_w_0()
                .child(Label::new(label).size(LabelSize::Small))
                .when_some(description, |this, description| {
                    this.child(
                        Label::new(description)
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
                }),
        )
}

fn option_marker(number: usize, chosen: Option<bool>) -> impl IntoElement {
    h_flex()
        .flex_none()
        .w(px(30.))
        .gap_0p5()
        .child(
            Label::new(format!("{number}."))
                .size(LabelSize::Small)
                .color(Color::Muted),
        )
        .when_some(chosen, |this, chosen| {
            this.child(
                Icon::new(if chosen {
                    IconName::Check
                } else {
                    IconName::Circle
                })
                .size(IconSize::XSmall)
                .color(if chosen {
                    Color::Accent
                } else {
                    Color::Disabled
                }),
            )
        })
}

fn chip(
    id: impl Into<ElementId>,
    icon: IconName,
    label: String,
    on_remove: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let colors = cx.theme().colors();
    let id = id.into();
    h_flex()
        .id(id.clone())
        .gap_1()
        .pl_1()
        .rounded_sm()
        .border_1()
        .border_color(colors.border_variant)
        .bg(colors.element_background)
        .child(Icon::new(icon).size(IconSize::XSmall).color(Color::Muted))
        .child(Label::new(label).size(LabelSize::XSmall))
        .child(
            IconButton::new(("remove", id_hash(&id)), IconName::Close)
                .icon_size(IconSize::XSmall)
                .on_click(on_remove),
        )
}

fn id_hash(id: &ElementId) -> usize {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    id.to_string().hash(&mut hasher);
    hasher.finish() as usize
}

const COMMAND_LIST_MAX_HEIGHT: f32 = 240.;
const MENTION_MAX_RESULTS: usize = 50;

struct MentionState {
    /// The bytes the chosen path replaces: from "@" to the end of the word.
    range: Range<usize>,
    query: String,
    entries: Vec<CommandEntry>,
}

#[derive(Clone)]
struct CommandEntry {
    label: SharedString,
    description: SharedString,
    kind: CommandKind,
}

#[derive(Clone)]
enum CommandKind {
    /// A CLI slash command, inserted as `/name ` for the person to send.
    Slash(String),
    /// A panel action, run at once.
    Action(fn() -> Box<dyn Action>),
    /// A file or folder, inserted as `@path ` in place of the "@" being typed.
    Mention {
        /// Relative to the session's directory.
        path: String,
        is_dir: bool,
        positions: Vec<usize>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum DiffLine {
    Context,
    Removed,
    Added,
}

/// The lines an Edit or Write call changes, with the lines both sides share
/// at the start and the end kept as context, as a diff shows them.
fn edit_diff(call: &ToolCall) -> Option<Vec<(DiffLine, String)>> {
    let field = |name: &str| call.input.get(name).and_then(Value::as_str);
    match call.name.as_str() {
        "Edit" => Some(diff_lines(field("old_string")?, field("new_string")?)),
        "Write" => Some(diff_lines("", field("content")?)),
        _ => None,
    }
}

fn diff_lines(old: &str, new: &str) -> Vec<(DiffLine, String)> {
    let old: Vec<&str> = if old.is_empty() {
        Vec::new()
    } else {
        old.lines().collect()
    };
    let new: Vec<&str> = if new.is_empty() {
        Vec::new()
    } else {
        new.lines().collect()
    };
    let prefix = old
        .iter()
        .zip(&new)
        .take_while(|(old, new)| old == new)
        .count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(old, new)| old == new)
        .count();
    let line = |kind: DiffLine| move |text: &&str| (kind, text.to_string());
    old[..prefix]
        .iter()
        .map(line(DiffLine::Context))
        .chain(
            old[prefix..old.len() - suffix]
                .iter()
                .map(line(DiffLine::Removed)),
        )
        .chain(
            new[prefix..new.len() - suffix]
                .iter()
                .map(line(DiffLine::Added)),
        )
        .chain(
            old[old.len() - suffix..]
                .iter()
                .map(line(DiffLine::Context)),
        )
        .collect()
}

fn diff_counts(diff: &[(DiffLine, String)]) -> (usize, usize) {
    let count = |kind| diff.iter().filter(|(line, _)| *line == kind).count();
    (count(DiffLine::Added), count(DiffLine::Removed))
}

/// The "@" mention being typed at byte `cursor`: the range from "@" to the
/// end of the word under the cursor, and what lies between "@" and the
/// cursor. The "@" must start the message or follow whitespace, so that an
/// address like name@example.com does not open the list.
fn mention_query(text: &str, cursor: usize) -> Option<(Range<usize>, &str)> {
    let before = text.get(..cursor)?;
    let at = before.rfind(|c: char| c == '@' || c.is_whitespace())?;
    let query = before[at..].strip_prefix('@')?;
    if before[..at]
        .chars()
        .next_back()
        .is_some_and(|c| !c.is_whitespace())
    {
        return None;
    }
    let after = &text[cursor..];
    let end = cursor + after.find(char::is_whitespace).unwrap_or(after.len());
    Some((at..end, query))
}

/// What replaces the "@" being typed: the mention, folders ending in "/",
/// followed by a space unless one is already there.
fn mention_insertion(text: &str, range: &Range<usize>, path: &str, is_dir: bool) -> String {
    let path = if is_dir {
        format!("{}/", path.trim_end_matches('/'))
    } else {
        path.to_string()
    };
    let mention = mention_text(&path, None);
    let followed_by_space = text
        .get(range.end..)
        .and_then(|rest| rest.chars().next())
        .is_some_and(char::is_whitespace);
    if followed_by_space {
        mention
    } else {
        format!("{mention} ")
    }
}

/// What follows "/" while the message is a command being typed: it starts
/// with "/" and has no space yet.
fn command_query(text: &str) -> Option<&str> {
    let query = text.strip_prefix('/')?;
    (!query.contains(char::is_whitespace)).then_some(query)
}

/// Slash commands matching `query`, those starting with it first, then the
/// panel's own actions whose name contains it.
fn matching_commands(query: &str, slash_commands: &[(String, String)]) -> Vec<CommandEntry> {
    let query = query.to_lowercase();
    let mut starts = Vec::new();
    let mut contains = Vec::new();
    for (name, description) in slash_commands {
        let lowercase = name.to_lowercase();
        let entry = CommandEntry {
            label: format!("/{name}").into(),
            description: description.clone().into(),
            kind: CommandKind::Slash(name.clone()),
        };
        if lowercase.starts_with(&query) {
            starts.push(entry);
        } else if lowercase.contains(&query) {
            contains.push(entry);
        }
    }
    let panel_actions: [(&str, &str, fn() -> Box<dyn Action>); 5] = [
        ("New conversation", "Start over in this view", || {
            NewConversation.boxed_clone()
        }),
        ("Past conversations", "Resume an earlier session", || {
            crate::ShowSessions.boxed_clone()
        }),
        ("Toggle focus view", "Show only prompts and replies", || {
            ToggleFocusView.boxed_clone()
        }),
        (
            "Toggle thinking",
            "Turn extended thinking on or off",
            || ToggleThinking.boxed_clone(),
        ),
        ("Show logs", "Open the Claude Code logs", || {
            crate::ShowLogs.boxed_clone()
        }),
    ];
    let actions = panel_actions
        .into_iter()
        .filter(|(label, _, _)| label.to_lowercase().contains(&query))
        .map(|(label, description, action)| CommandEntry {
            label: label.into(),
            description: description.into(),
            kind: CommandKind::Action(action),
        });
    starts.into_iter().chain(contains).chain(actions).collect()
}

/// The modes the mode menu and Shift+Tab offer. Auto is always offered: the
/// CLI does not say whether an account can use it until it is asked, and a
/// conversation that did not start in Auto must still be able to switch to it.
fn available_modes(allow_bypass_permissions: bool) -> Vec<PermissionMode> {
    let mut modes = vec![
        PermissionMode::Auto,
        PermissionMode::Default,
        PermissionMode::AcceptEdits,
        PermissionMode::Plan,
    ];
    if allow_bypass_permissions {
        modes.push(PermissionMode::BypassPermissions);
    }
    modes
}

fn mode_icon(mode: PermissionMode) -> IconName {
    match mode {
        PermissionMode::Auto => IconName::BoltFilled,
        PermissionMode::Default | PermissionMode::DontAsk => IconName::Eye,
        PermissionMode::AcceptEdits => IconName::Pencil,
        PermissionMode::Plan => IconName::ListTodo,
        PermissionMode::BypassPermissions => IconName::Warning,
    }
}

pub fn is_edit_tool(name: &str) -> bool {
    matches!(name, "Edit" | "MultiEdit" | "Write" | "NotebookEdit")
}

pub fn display_tool_name(name: &str) -> String {
    match name.strip_prefix("mcp__") {
        Some(rest) => rest.replacen("__", ": ", 1),
        None => name.to_string(),
    }
}

pub fn tool_input_text(call: &ToolCall) -> String {
    let field = |name: &str| call.input.get(name).and_then(Value::as_str);
    match call.name.as_str() {
        "Bash" => field("command").unwrap_or_default().to_string(),
        "Read" | "Write" | "Edit" | "MultiEdit" => {
            let path = field("file_path").unwrap_or_default();
            match (field("old_string"), field("new_string"), field("content")) {
                (Some(old), Some(new), _) => format!(
                    "{path}\n- {}\n+ {}",
                    truncate_lines(old, 6),
                    truncate_lines(new, 6)
                ),
                (_, _, Some(content)) => format!("{path}\n{}", truncate_lines(content, 8)),
                _ => path.to_string(),
            }
        }
        _ => {
            if call.input.is_null() {
                call.partial_input.clone()
            } else {
                serde_json::to_string_pretty(&call.input).unwrap_or_default()
            }
        }
    }
}

/// The notice shown after "Rewind code to here", from the CLI's answer
/// (`canRewind`, `error`, `filesChanged`, `insertions`, `deletions`).
fn describe_rewind(response: &serde_json::Value) -> (NoticeKind, String) {
    if response
        .get("canRewind")
        .and_then(serde_json::Value::as_bool)
        == Some(false)
    {
        let reason = response
            .get("error")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("no checkpoint for that message");
        return (
            NoticeKind::Error,
            format!("Could not rewind the code: {reason}"),
        );
    }
    let files = response
        .get("filesChanged")
        .and_then(serde_json::Value::as_array)
        .map_or(0, Vec::len);
    let count = |key: &str| {
        response
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    };
    let text = if files == 0 {
        "Rewound the code: no files needed changing.".to_string()
    } else {
        format!(
            "Rewound the code: {files} file{} restored (+{} −{}).",
            if files == 1 { "" } else { "s" },
            count("insertions"),
            count("deletions")
        )
    };
    (NoticeKind::Info, text)
}

pub fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut truncated: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    truncated.push('…');
    truncated
}

pub fn truncate_lines(text: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= max_lines {
        return text.to_string();
    }
    format!(
        "{}\n… {} more lines",
        lines[..max_lines].join("\n"),
        lines.len() - max_lines
    )
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn format_duration(seconds: i64) -> String {
    if seconds >= 3600 {
        format!("{}h {}m", seconds / 3600, (seconds % 3600) / 60)
    } else {
        format!("{}m", (seconds / 60).max(1))
    }
}

fn placeholder(suggestion: Option<&str>) -> &str {
    suggestion.unwrap_or("Ask Claude anything. @ to mention files, / for commands")
}

/// `@path`, or `@path#start-end` with 1-based rows, the form the CLI expands.
pub fn mention_text(path: &str, rows: Option<(u32, u32)>) -> String {
    let path = if path.contains(' ') {
        format!("\"{path}\"")
    } else {
        path.to_string()
    };
    match rows {
        Some((start, end)) if start == end => format!("@{path}#{}", start + 1),
        Some((start, end)) => format!("@{path}#{}-{}", start + 1, end + 1),
        None => format!("@{path}"),
    }
}

const CONTEXT_OPEN: &str = "<ide_context>";
const CONTEXT_CLOSE: &str = "</ide_context>\n";

/// What the open file and selection add to a prompt, in the tags the CLI
/// uses for IDE context, so that Claude weighs it the same way.
pub fn context_preamble(context: &EditorContext) -> String {
    let path = context.path.to_string_lossy();
    let body = match &context.selection {
        Some((start, end, text)) => format!(
            "<ide_selection>The user selected the lines {} to {} from {path}:\n{text}\n\nThis may or may not be related to the current task.</ide_selection>",
            start + 1,
            end + 1
        ),
        None => format!(
            "<ide_opened_file>The user opened the file {path} in the IDE. This may or may not be related to the current task.</ide_opened_file>"
        ),
    };
    format!("{CONTEXT_OPEN}{body}{CONTEXT_CLOSE}")
}

pub fn strip_context_preamble(text: &str) -> &str {
    match (text.strip_prefix(CONTEXT_OPEN), text.find(CONTEXT_CLOSE)) {
        (Some(_), Some(end)) => &text[end + CONTEXT_CLOSE.len()..],
        _ => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use util::path;

    #[test]
    fn slash_opens_the_command_list() {
        assert_eq!(command_query("/"), Some(""));
        assert_eq!(command_query("/com"), Some("com"));
        assert_eq!(command_query("/compact now"), None);
        assert_eq!(command_query("hello /x"), None);

        let commands = vec![
            ("review".to_string(), "Review a PR".to_string()),
            ("compact".to_string(), "Compact the context".to_string()),
            ("pr-comments".to_string(), String::new()),
        ];
        let labels = |query: &str| {
            matching_commands(query, &commands)
                .into_iter()
                .map(|entry| entry.label.to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            labels("")[..3],
            ["/review", "/compact", "/pr-comments"].map(String::from)
        );
        assert_eq!(labels("").len(), 3 + 5);
        assert_eq!(
            labels("co"),
            [
                "/compact",
                "/pr-comments",
                "New conversation",
                "Past conversations"
            ]
            .map(String::from)
        );
        assert_eq!(
            labels("CONV"),
            ["New conversation", "Past conversations"].map(String::from)
        );
        assert!(labels("zzz").is_empty());
    }

    #[test]
    fn at_sign_opens_the_mention_list() {
        assert_eq!(mention_query("@", 1), Some((0..1, "")));
        assert_eq!(mention_query("see @src/ma", 11), Some((4..11, "src/ma")));
        // The cursor in the middle of the message, and of the word: the query
        // stops at the cursor, the replaced range runs to the end of the word.
        let text = "see @src/main.rs and more";
        assert_eq!(mention_query(text, 11), Some((4..16, "src/ma")));
        assert_eq!(mention_query(text, 20), None);
        assert_eq!(mention_query("line one\n@doc", 13), Some((9..13, "doc")));
        // Glued to a word, as in an email address: no list.
        assert_eq!(mention_query("mail stan@example.com", 21), None);
        assert_eq!(mention_query("a@", 2), None);
        assert_eq!(mention_query("@@", 2), None);
        // A finished mention closes it.
        assert_eq!(mention_query("@src/main.rs ", 13), None);
        assert_eq!(mention_query("no mention", 10), None);
        assert_eq!(
            mention_query("héllo @é", "héllo @é".len()),
            Some((7..10, "é"))
        );
        assert_eq!(mention_query("abc", 10), None);
    }

    #[test]
    fn edit_diffs_keep_shared_lines_as_context() {
        let diff = diff_lines(
            "def greet(name):\n    return name\n",
            "def greet(name: str) -> str:\n    return name\n",
        );
        assert_eq!(
            diff,
            vec![
                (DiffLine::Removed, "def greet(name):".to_string()),
                (DiffLine::Added, "def greet(name: str) -> str:".to_string()),
                (DiffLine::Context, "    return name".to_string()),
            ]
        );
        assert_eq!(diff_counts(&diff), (1, 1));
        let diff = diff_lines("a\nb\nc", "a\nc");
        assert_eq!(
            diff,
            vec![
                (DiffLine::Context, "a".to_string()),
                (DiffLine::Removed, "b".to_string()),
                (DiffLine::Context, "c".to_string()),
            ]
        );
        assert_eq!(
            diff_lines("", "new\nfile"),
            vec![
                (DiffLine::Added, "new".to_string()),
                (DiffLine::Added, "file".to_string()),
            ]
        );
        // Identical sides: all context, nothing counted twice.
        assert_eq!(diff_counts(&diff_lines("x\ny", "x\ny")), (0, 0));
        assert_eq!(diff_lines("x\ny", "x\ny").len(), 2);
    }

    #[test]
    fn mention_insertion_text() {
        assert_eq!(
            mention_insertion("see @src/ma", &(4..11), "src/main.rs", false),
            "@src/main.rs "
        );
        assert_eq!(mention_insertion("@sr", &(0..3), "src", true), "@src/ ");
        assert_eq!(
            mention_insertion("@a and b", &(0..2), "my dir/a.rs", false),
            "@\"my dir/a.rs\""
        );
    }

    #[gpui::test]
    async fn typing_at_lists_and_inserts_project_paths(cx: &mut gpui::TestAppContext) {
        let app_state = cx.update(|cx| {
            let state = workspace::AppState::test(cx);
            theme_settings::init(theme::LoadThemes::JustBase, cx);
            editor::init(cx);
            ClaudeCodeSettings::register(cx);
            state
        });
        let fs = app_state.fs.as_fake();
        fs.insert_tree(
            path!("/root"),
            serde_json::json!({
                "src": { "main.rs": "", "lib.rs": "" },
                "docs": { "src-map.md": "" },
                "README.md": "",
            }),
        )
        .await;
        let project =
            project::Project::test(app_state.fs.clone(), [path!("/root").as_ref()], cx).await;
        let (multi_workspace, cx) = cx
            .add_window_view(|window, cx| workspace::MultiWorkspace::test_new(project, window, cx));
        let workspace =
            multi_workspace.read_with(cx, |multi_workspace, _| multi_workspace.workspace().clone());
        let view = workspace.update_in(cx, |workspace, window, cx| {
            let project = workspace.project().clone();
            let workspace = cx.entity();
            crate::panel::new_view(&workspace, project, None, window, cx)
        });
        cx.run_until_parked();

        let labels = |view: &Entity<ClaudeCodeView>, cx: &mut gpui::VisualTestContext| {
            view.read_with(cx, |view, cx| {
                view.visible_commands(cx)
                    .into_iter()
                    .map(|entry| entry.label.to_string())
                    .collect::<Vec<_>>()
            })
        };

        view.update_in(cx, |view, window, cx| {
            view.input
                .update(cx, |input, cx| input.insert("see @", window, cx));
        });
        cx.run_until_parked();
        // No query: the first files.
        let mut first_files = labels(&view, cx);
        first_files.sort();
        assert_eq!(
            first_files,
            ["README.md", "docs/src-map.md", "src/lib.rs", "src/main.rs"]
        );

        view.update_in(cx, |view, window, cx| {
            view.input
                .update(cx, |input, cx| input.insert("src/ma", window, cx));
        });
        cx.run_until_parked();
        assert_eq!(labels(&view, cx)[0], "src/main.rs");

        view.update_in(cx, |view, window, cx| {
            view.input
                .update(cx, |input, cx| input.set_text("see @sr", window, cx));
            view.input.update(cx, |input, cx| {
                input.move_to_end(&editor::actions::MoveToEnd, window, cx)
            });
        });
        cx.run_until_parked();
        assert_eq!(labels(&view, cx)[0], "src/");
        view.update_in(cx, |view, window, cx| {
            view.confirm_command(&ConfirmCommand, window, cx);
            assert_eq!(view.input.read(cx).text(cx), "see @src/ ");
        });
        cx.run_until_parked();
        assert!(labels(&view, cx).is_empty());

        // Escape closes the list until the mention is finished.
        view.update_in(cx, |view, window, cx| {
            view.input
                .update(cx, |input, cx| input.insert("@READ", window, cx));
        });
        cx.run_until_parked();
        assert_eq!(labels(&view, cx), ["README.md"]);
        view.update_in(cx, |view, window, cx| {
            view.dismiss_commands(&DismissCommands, window, cx);
        });
        assert!(labels(&view, cx).is_empty());
        view.update_in(cx, |view, window, cx| {
            view.input
                .update(cx, |input, cx| input.insert(" @", window, cx));
        });
        cx.run_until_parked();
        assert!(!labels(&view, cx).is_empty());
        // An email address does not open it.
        view.update_in(cx, |view, window, cx| {
            view.input.update(cx, |input, cx| {
                input.set_text("write to me@exa", window, cx)
            });
        });
        cx.run_until_parked();
        assert!(labels(&view, cx).is_empty());
    }

    #[test]
    fn auto_mode_is_always_offered() {
        let modes = available_modes(false);
        assert_eq!(
            modes,
            vec![
                PermissionMode::Auto,
                PermissionMode::Default,
                PermissionMode::AcceptEdits,
                PermissionMode::Plan,
            ]
        );
        assert_eq!(
            available_modes(true).last(),
            Some(&PermissionMode::BypassPermissions)
        );
    }

    #[test]
    fn mentions_use_one_based_rows() {
        assert_eq!(mention_text("src/a.rs", None), "@src/a.rs");
        assert_eq!(mention_text("src/a.rs", Some((4, 9))), "@src/a.rs#5-10");
        assert_eq!(mention_text("src/a.rs", Some((4, 4))), "@src/a.rs#5");
        assert_eq!(mention_text("my dir/a.rs", None), "@\"my dir/a.rs\"");
    }

    #[test]
    fn context_preamble_round_trips() {
        let context = EditorContext {
            path: PathBuf::from("/p/a.rs"),
            selection: Some((1, 2, "fn a() {}".into())),
        };
        let prompt = format!("{}explain this", context_preamble(&context));
        assert!(prompt.contains("lines 2 to 3 from /p/a.rs"));
        assert_eq!(strip_context_preamble(&prompt), "explain this");
        assert_eq!(strip_context_preamble("plain"), "plain");
        let opened = context_preamble(&EditorContext {
            path: PathBuf::from("/p/a.rs"),
            selection: None,
        });
        assert!(opened.contains("<ide_opened_file>"));
    }

    #[test]
    fn tool_names_and_truncation() {
        assert_eq!(
            display_tool_name("mcp__vireo__getDiagnostics"),
            "vireo: getDiagnostics"
        );
        assert_eq!(display_tool_name("Bash"), "Bash");
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate_lines("a\nb\nc", 2), "a\nb\n… 1 more lines");
        assert_eq!(format_duration(59 * 60 + 5), "59m");
        assert_eq!(format_duration(3 * 3600 + 120), "3h 2m");
    }

    #[test]
    fn rewind_notices() {
        assert_eq!(
            describe_rewind(
                &serde_json::json!({"canRewind": true, "filesChanged": ["/a", "/b"], "insertions": 3, "deletions": 1})
            ),
            (
                NoticeKind::Info,
                "Rewound the code: 2 files restored (+3 −1).".to_string()
            )
        );
        assert_eq!(
            describe_rewind(&serde_json::json!({"canRewind": false, "error": "No checkpoint"})),
            (
                NoticeKind::Error,
                "Could not rewind the code: No checkpoint".to_string()
            )
        );
        assert_eq!(
            describe_rewind(&serde_json::json!({"canRewind": true})).0,
            NoticeKind::Info
        );
    }

    #[gpui::test]
    async fn view_renders_entries_and_answers_permissions(cx: &mut gpui::TestAppContext) {
        let app_state = cx.update(|cx| {
            let state = workspace::AppState::test(cx);
            theme_settings::init(theme::LoadThemes::JustBase, cx);
            editor::init(cx);
            ClaudeCodeSettings::register(cx);
            state
        });
        let project = project::Project::test(app_state.fs.clone(), [], cx).await;
        let (multi_workspace, cx) = cx
            .add_window_view(|window, cx| workspace::MultiWorkspace::test_new(project, window, cx));
        let workspace =
            multi_workspace.read_with(cx, |multi_workspace, _| multi_workspace.workspace().clone());
        let view = workspace.update_in(cx, |workspace, window, cx| {
            let project = workspace.project().clone();
            let workspace = cx.entity();
            crate::panel::new_view(&workspace, project, None, window, cx)
        });
        let session = view.read_with(cx, |view, _| view.session.clone());
        session.update(cx, |session, _| {
            session
                .conversation
                .push_user_prompt("list files".into(), 0, Utc::now());
            for line in [
                r#"{"type":"assistant","message":{"id":"m1","content":[{"type":"thinking","thinking":"hm"},{"type":"text","text":"Sure, **listing**."},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls","description":"List files"}}]}}"#,
                r#"{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls"},"permission_suggestions":[{"type":"addRules"}]}}"#,
            ] {
                session
                    .conversation
                    .apply(crate::protocol::CliMessage::parse(line).unwrap(), Utc::now());
            }
        });
        cx.run_until_parked();

        view.update_in(cx, |view, window, cx| {
            assert_eq!(view.title(cx), "list files");
            assert_eq!(view.render_entries(window, cx).len(), 5);
            view.focus_view = true;
            // Focus view keeps the prompt, the reply and the pending question.
            assert_eq!(view.render_entries(window, cx).len(), 3);
            view.focus_view = false;
            view.allow_permission(&AllowPermission, window, cx);
        });
        session.read_with(cx, |session, _| {
            assert_eq!(session.conversation.pending_permissions().count(), 0);
            assert!(session.conversation.entries.iter().any(|entry| matches!(
                entry,
                Entry::Permission(request) if request.status == PermissionStatus::Allowed
            )));
        });
        let _ = cx.draw(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(400.), px(800.)),
            |_, _| div().size_full().child(view.clone()),
        );
    }

    /// A Claude view open in a tab of a test workspace, drawn in its window
    /// so that keystrokes reach it, with the default keymap bound.
    async fn view_in_tab(
        cx: &mut gpui::TestAppContext,
        entries: Vec<Entry>,
    ) -> (Entity<ClaudeCodeView>, &mut gpui::VisualTestContext) {
        let app_state = cx.update(|cx| {
            let state = workspace::AppState::test(cx);
            theme_settings::init(theme::LoadThemes::JustBase, cx);
            editor::init(cx);
            ClaudeCodeSettings::register(cx);
            let bindings = settings::KeymapFile::load_asset_allow_partial_failure(
                settings::DEFAULT_KEYMAP_PATH,
                cx,
            )
            .unwrap_or_default();
            cx.bind_keys(bindings);
            state
        });
        let project = project::Project::test(app_state.fs.clone(), [], cx).await;
        let (multi_workspace, cx) = cx
            .add_window_view(|window, cx| workspace::MultiWorkspace::test_new(project, window, cx));
        let workspace =
            multi_workspace.read_with(cx, |multi_workspace, _| multi_workspace.workspace().clone());
        // Built by hand rather than with `panel::open`, so that the session can
        // start with a transcript the way a resumed one does.
        let view = workspace.update_in(cx, |workspace, window, cx| {
            let project = workspace.project().clone();
            let workspace_entity = cx.entity();
            let session = cx.new(|cx| {
                let mut session =
                    ClaudeSession::new(project.downgrade(), PathBuf::from("/"), None, cx);
                session.conversation.entries = entries;
                session
            });
            let view = cx.new(|cx| {
                ClaudeCodeView::new(session, workspace_entity.downgrade(), project, window, cx)
            });
            let tab = cx.new(|cx| {
                crate::panel::ClaudeCodeTab::new(workspace_entity.downgrade(), view.clone(), cx)
            });
            workspace.add_item_to_active_pane(Box::new(tab), None, true, window, cx);
            window.focus(&view.read(cx).input_focus_handle(cx), cx);
            view
        });
        cx.run_until_parked();
        (view, cx)
    }

    fn long_transcript() -> Vec<Entry> {
        let mut conversation = crate::conversation::Conversation::default();
        for index in 0..80 {
            conversation.push_user_prompt(format!("prompt number {index}"), 0, Utc::now());
        }
        conversation.entries
    }

    fn scroll_position(
        view: &Entity<ClaudeCodeView>,
        cx: &mut gpui::VisualTestContext,
    ) -> (gpui::Pixels, gpui::Pixels) {
        view.read_with(cx, |view, _| {
            (
                -view.scroll_handle.offset().y,
                view.scroll_handle.max_offset().y,
            )
        })
    }

    #[gpui::test]
    async fn a_resumed_conversation_opens_on_its_latest_messages(cx: &mut gpui::TestAppContext) {
        let (view, cx) = view_in_tab(cx, long_transcript()).await;
        cx.run_until_parked();
        let (offset, max) = scroll_position(&view, cx);
        assert!(max > px(0.), "the transcript overflows the list");
        assert_eq!(offset, max, "scrolled to the bottom");
        view.read_with(cx, |view, _| {
            assert!(view.follow_tail);
            assert!(!view.shows_jump_to_bottom());
        });

        // New messages are followed.
        let session = view.read_with(cx, |view, _| view.session.clone());
        session.update(cx, |session, cx| {
            session.conversation.push_user_prompt(
                "one more\nwith\nseveral\nlines".into(),
                0,
                Utc::now(),
            );
            cx.emit(SessionEvent::Updated);
        });
        cx.run_until_parked();
        let (offset, new_max) = scroll_position(&view, cx);
        assert!(new_max > max);
        assert_eq!(offset, new_max);
    }

    #[gpui::test]
    async fn the_jump_button_shows_while_scrolled_up(cx: &mut gpui::TestAppContext) {
        let (view, cx) = view_in_tab(cx, long_transcript()).await;
        cx.run_until_parked();
        // Scrolling up with the wheel stops following and shows the button.
        let bounds = view.read_with(cx, |view, _| view.scroll_handle.bounds());
        cx.simulate_event(gpui::ScrollEvent {
            position: bounds.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(400.))),
            ..Default::default()
        });
        cx.run_until_parked();
        let (offset, max) = scroll_position(&view, cx);
        assert!(offset < max);
        view.read_with(cx, |view, _| {
            assert!(!view.follow_tail);
            assert!(view.shows_jump_to_bottom());
        });

        // Clicking it goes back to the bottom and follows again.
        let button = cx
            .debug_bounds("jump-to-bottom")
            .expect("the jump button is drawn");
        cx.simulate_click(button.center(), gpui::Modifiers::none());
        cx.run_until_parked();
        let (offset, max) = scroll_position(&view, cx);
        assert_eq!(offset, max);
        view.read_with(cx, |view, _| {
            assert!(view.follow_tail);
            assert!(!view.shows_jump_to_bottom());
        });
    }

    #[gpui::test]
    async fn no_jump_button_when_the_list_fits(cx: &mut gpui::TestAppContext) {
        let mut conversation = crate::conversation::Conversation::default();
        conversation.push_user_prompt("short".into(), 0, Utc::now());
        let (view, cx) = view_in_tab(cx, conversation.entries).await;
        cx.run_until_parked();
        view.update(cx, |view, _| view.follow_tail = false);
        cx.run_until_parked();
        view.read_with(cx, |view, _| assert!(!view.shows_jump_to_bottom()));
    }

    #[gpui::test]
    async fn your_messages_are_selectable_and_copied_as_typed(cx: &mut gpui::TestAppContext) {
        let typed = "keep *this* as typed\n  indented line\n# not a heading";
        let context = context_preamble(&EditorContext {
            path: PathBuf::from("/p/a.rs"),
            selection: None,
        });
        let mut conversation = crate::conversation::Conversation::default();
        conversation.push_user_prompt(format!("{context}{typed}"), 0, Utc::now());
        conversation.apply(
            crate::protocol::CliMessage::parse(
                r#"{"type":"assistant","message":{"id":"m1","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"echo *hi*"}}]}}"#,
            )
            .unwrap(),
            Utc::now(),
        );
        let (view, cx) = view_in_tab(cx, conversation.entries).await;
        cx.run_until_parked();
        view.read_with(cx, |view, cx| {
            // Drawn with the selectable element, from the text as typed: no
            // context preamble, nothing read as Markdown.
            let (_, message) = &view.plain_text[&(0, PlainPart::Message)];
            assert_eq!(message.read(cx).source().as_ref(), typed);
            let (_, input) = &view.plain_text[&(1, PlainPart::ToolInput)];
            assert_eq!(input.read(cx).source().as_ref(), "echo *hi*");
        });
        view.update(cx, |view, cx| view.copy_user_message(0, cx));
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some(typed.to_string())
        );
    }

    fn request_permission(
        view: &Entity<ClaudeCodeView>,
        line: &str,
        cx: &mut gpui::VisualTestContext,
    ) {
        let session = view.read_with(cx, |view, _| view.session.clone());
        let message = crate::protocol::CliMessage::parse(line).unwrap();
        session.update(cx, |session, cx| {
            session.conversation.apply(message, Utc::now());
            let request_id = session
                .conversation
                .pending_permissions()
                .last()
                .map(|request| request.request_id.clone())
                .unwrap_or_default();
            cx.emit(SessionEvent::PermissionRequested { request_id });
        });
        cx.run_until_parked();
    }

    #[gpui::test]
    async fn permission_options_are_chosen_with_the_keyboard(cx: &mut gpui::TestAppContext) {
        let (view, cx) = view_in_tab(cx, Vec::new()).await;
        request_permission(
            &view,
            r#"{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls"},"permission_suggestions":[{"type":"addRules"}]}}"#,
            cx,
        );
        view.update_in(cx, |view, window, _| {
            assert!(
                view.card_focus.is_focused(window),
                "the card takes the keys"
            );
            assert_eq!(view.card_selection, 0);
        });
        cx.simulate_keystrokes("down down");
        view.read_with(cx, |view, _| assert_eq!(view.card_selection, 2));
        // Past the last option the highlight wraps to the first.
        cx.simulate_keystrokes("down");
        view.read_with(cx, |view, _| assert_eq!(view.card_selection, 0));
        cx.simulate_keystrokes("down enter");
        view.update_in(cx, |view, window, cx| {
            assert_eq!(
                view.last_decision,
                Some(serde_json::json!({
                    "behavior": "allow",
                    "updatedInput": {"command": "ls"},
                    "updatedPermissions": [{"type": "addRules"}],
                }))
            );
            assert!(
                view.input.focus_handle(cx).is_focused(window),
                "back to the message box"
            );
        });

        // Number keys choose directly: 2 is "No" when there is no "always".
        request_permission(
            &view,
            r#"{"type":"control_request","request_id":"r2","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"rm x"}}}"#,
            cx,
        );
        cx.simulate_keystrokes("2");
        view.read_with(cx, |view, _| {
            assert_eq!(
                view.last_decision
                    .as_ref()
                    .and_then(|decision| decision.get("behavior")),
                Some(&serde_json::json!("deny"))
            );
        });
    }

    const ASK_TWO_QUESTIONS: &str = r#"{"type":"control_request","request_id":"q1","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[{"question":"Which database?","header":"Database","multiSelect":false,"options":[{"label":"Postgres","description":"Relational"},{"label":"SQLite","description":"A file"}]},{"question":"Which extras?","header":"Extras","multiSelect":true,"options":[{"label":"Docker","description":""},{"label":"CI","description":"GitHub Actions"}]}]}}}"#;

    #[test]
    fn question_input_and_answers() {
        let input: Value =
            serde_json::from_str::<Value>(ASK_TWO_QUESTIONS).unwrap()["request"]["input"].clone();
        let questions = parse_questions(&input);
        assert_eq!(questions.len(), 2);
        assert_eq!(questions[0].header, "Database");
        assert_eq!(questions[0].options[1].description, "A file");
        assert!(questions[1].multi_select);
        let chosen = |options: &[usize]| {
            options
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
        };
        assert_eq!(
            question_answers(
                &questions,
                &[chosen(&[1]), chosen(&[0, 2])],
                &[String::new(), " mine ".into()]
            ),
            serde_json::json!({"Which database?": "SQLite", "Which extras?": "Docker, mine"})
                .as_object()
                .cloned()
        );
        // A question left unanswered, or "Other" chosen with nothing typed.
        assert_eq!(
            question_answers(
                &questions,
                &[chosen(&[1]), chosen(&[])],
                &[String::new(), String::new()]
            ),
            None
        );
        assert_eq!(
            question_answers(
                &questions,
                &[chosen(&[1]), chosen(&[2])],
                &[String::new(), "  ".into()]
            ),
            None
        );
    }

    #[gpui::test]
    async fn ask_user_question_is_answered_from_its_options(cx: &mut gpui::TestAppContext) {
        let (view, cx) = view_in_tab(cx, Vec::new()).await;
        request_permission(&view, ASK_TWO_QUESTIONS, cx);
        view.update_in(cx, |view, window, _| {
            assert!(view.question_form.is_some());
            assert!(view.card_focus.is_focused(window));
        });
        // "2" picks SQLite and moves on to the second question.
        cx.simulate_keystrokes("2");
        view.read_with(cx, |view, _| {
            let form = view.question_form.as_ref().unwrap();
            assert_eq!(form.chosen[0], [1].into_iter().collect());
            assert_eq!(form.cursor, form.first_row_of(1));
        });
        // Enter checks Docker; "3" goes to the second question's Other field.
        cx.simulate_keystrokes("enter 3");
        cx.simulate_input("Kubernetes");
        view.read_with(cx, |view, cx| {
            let form = view.question_form.as_ref().unwrap();
            assert_eq!(form.other[1].read(cx).text(cx), "Kubernetes");
            assert_eq!(form.chosen[1], [0, 2].into_iter().collect());
            assert!(view.last_decision.is_none());
        });
        cx.simulate_keystrokes("enter");
        view.read_with(cx, |view, cx| {
            let input: Value = serde_json::from_str::<Value>(ASK_TWO_QUESTIONS).unwrap()["request"]
                ["input"]
                .clone();
            let mut expected = input;
            expected["answers"] = serde_json::json!({
                "Which database?": "SQLite",
                "Which extras?": "Docker, Kubernetes",
            });
            assert_eq!(
                view.last_decision,
                Some(serde_json::json!({"behavior": "allow", "updatedInput": expected}))
            );
            assert!(view.question_form.is_none());
            assert_eq!(
                view.session
                    .read(cx)
                    .conversation
                    .pending_permissions()
                    .count(),
                0
            );
        });

        // Escape dismisses, telling Claude so, without interrupting the turn.
        request_permission(&view, &ASK_TWO_QUESTIONS.replace("\"q1\"", "\"q2\""), cx);
        cx.simulate_keystrokes("escape");
        view.read_with(cx, |view, _| {
            assert_eq!(
                view.last_decision,
                Some(serde_json::json!({"behavior": "deny", "message": DISMISSED_QUESTION}))
            );
        });
    }

    #[gpui::test]
    async fn tab_accepts_the_suggested_prompt(cx: &mut gpui::TestAppContext) {
        let (view, cx) = view_in_tab(cx, Vec::new()).await;
        let session = view.read_with(cx, |view, _| view.session.clone());
        session.update(cx, |session, cx| {
            session.conversation.prompt_suggestion = Some("run the tests".into());
            cx.emit(SessionEvent::Updated);
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("tab");
        view.update_in(cx, |view, window, cx| {
            assert_eq!(view.input.read(cx).text(cx), "run the tests");
            // The cursor ends up after the suggestion, so typing continues it.
            view.input
                .update(cx, |input, cx| input.insert("!", window, cx));
            assert_eq!(view.input.read(cx).text(cx), "run the tests!");
            view.input
                .update(cx, |input, cx| input.set_text("x", window, cx));
        });
        cx.run_until_parked();
        // With text in the box, Tab is the editor's own again.
        cx.simulate_keystrokes("tab");
        view.read_with(cx, |view, cx| {
            assert_ne!(view.input.read(cx).text(cx), "run the tests");
        });

        // The "/" list's keys, whose context had the same mistake.
        view.update_in(cx, |view, window, cx| {
            view.input
                .update(cx, |input, cx| input.set_text("/", window, cx));
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("down");
        view.read_with(cx, |view, _| assert_eq!(view.command_selection, 1));
    }

    /// V-15: the app's path to the suggestion, unlike the test above: the panel
    /// in the side bar, the VS Code base keymap on top of the default one, a
    /// prompt typed and sent, and the CLI's own lines for the reply, the result
    /// and the suggestion.
    #[gpui::test]
    async fn tab_accepts_a_suggestion_from_the_cli_in_the_side_bar(cx: &mut gpui::TestAppContext) {
        let app_state = cx.update(|cx| {
            let state = workspace::AppState::test(cx);
            theme_settings::init(theme::LoadThemes::JustBase, cx);
            editor::init(cx);
            ClaudeCodeSettings::register(cx);
            for path in [
                settings::DEFAULT_KEYMAP_PATH,
                "keymaps/macos/vscode.json",
                "keymaps/specific-overrides-macos.json",
            ] {
                let bindings = settings::KeymapFile::load_asset_allow_partial_failure(path, cx)
                    .unwrap_or_default();
                cx.bind_keys(bindings);
            }
            state
        });
        let project = project::Project::test(app_state.fs.clone(), [], cx).await;
        let (multi_workspace, cx) = cx
            .add_window_view(|window, cx| workspace::MultiWorkspace::test_new(project, window, cx));
        let workspace =
            multi_workspace.read_with(cx, |multi_workspace, _| multi_workspace.workspace().clone());
        let events = std::rc::Rc::new(std::cell::RefCell::new(None));
        let panel = workspace.update_in(cx, |workspace, window, cx| {
            let project = workspace.project().clone();
            let workspace_entity = cx.entity();
            let panel = cx.new(|cx| {
                crate::panel::ClaudeCodePanel::load_for_test(workspace_entity, project, window, cx)
            });
            workspace.add_panel(panel.clone(), window, cx);
            panel
        });
        let view = panel.read_with(cx, |panel, _| panel.view.clone());
        view.update(cx, |view, cx| {
            let events = events.clone();
            view.session.update(cx, |session, _| {
                session.set_fake_spawner(std::rc::Rc::new(move |_| {
                    let (process, events_rx, events_tx, stdin) =
                        crate::process::ClaudeProcess::fake();
                    *events.borrow_mut() = Some((events_tx, stdin));
                    (process, events_rx)
                }));
            });
        });
        workspace.update_in(cx, |workspace, window, cx| {
            crate::panel::open(workspace, crate::panel::Location::Panel, None, window, cx);
        });
        cx.run_until_parked();

        cx.simulate_input("Say hi in three words.");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let send = |line: &str, cx: &mut gpui::VisualTestContext| {
            let message = crate::protocol::CliMessage::parse(line).expect("a protocol line");
            events
                .borrow()
                .as_ref()
                .expect("the prompt started claude")
                .0
                .unbounded_send(crate::process::ProcessEvent::Message(message))
                .expect("the session reads events");
            cx.run_until_parked();
        };
        send(
            r#"{"type":"system","subtype":"init","session_id":"d742","model":"claude-haiku-5-5","slash_commands":[]}"#,
            cx,
        );
        send(
            r#"{"type":"assistant","message":{"model":"claude-haiku-5-5","id":"msg_1","type":"message","role":"assistant","content":[{"type":"text","text":"Hi there, friend!"}],"stop_reason":null},"parent_tool_use_id":null,"session_id":"d742","uuid":"025b"}"#,
            cx,
        );
        send(
            r#"{"duration_api_ms":2350,"stop_reason":"end_turn","session_id":"d742","total_cost_usd":0.0087,"type":"result","subtype":"success","is_error":false,"result":"Hi there, friend!","num_turns":1}"#,
            cx,
        );
        send(
            r#"{"type":"prompt_suggestion","suggestion":"run the tests","uuid":"7c1e","session_id":"d742"}"#,
            cx,
        );
        view.read_with(cx, |view, cx| {
            assert_eq!(
                view.session
                    .read(cx)
                    .conversation
                    .prompt_suggestion
                    .as_deref(),
                Some("run the tests")
            );
            assert_eq!(view.input.read(cx).text(cx), "");
        });
        cx.update(|window, cx| {
            assert!(
                view.read(cx).input.focus_handle(cx).is_focused(window),
                "the message box keeps the focus through the turn"
            );
        });

        cx.simulate_keystrokes("tab");
        view.read_with(cx, |view, cx| {
            assert_eq!(view.input.read(cx).text(cx), "run the tests");
        });
    }

    /// With prompt suggestions turned off, the suggestion is not shown, so Tab
    /// does not paste it.
    #[gpui::test]
    async fn tab_ignores_a_suggestion_that_is_not_shown(cx: &mut gpui::TestAppContext) {
        let (view, cx) = view_in_tab(cx, Vec::new()).await;
        cx.update(|_, cx| {
            cx.update_global::<settings::SettingsStore, ()>(|store, cx| {
                store.update_user_settings(cx, |settings| {
                    settings
                        .claude_code
                        .get_or_insert_default()
                        .prompt_suggestions = Some(false);
                });
            });
        });
        let session = view.read_with(cx, |view, _| view.session.clone());
        session.update(cx, |session, cx| {
            session.conversation.prompt_suggestion = Some("run the tests".into());
            cx.emit(SessionEvent::Updated);
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("tab");
        view.read_with(cx, |view, cx| {
            assert_ne!(view.input.read(cx).text(cx), "run the tests");
        });
    }
}
