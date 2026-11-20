use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use chrono::{DateTime, Local, Utc};
use editor::{Editor, EditorEvent, EditorMode, MultiBuffer};
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
    CommonAnimationExt as _, ContextMenu, ContextMenuEntry, Divider, IconButtonShape, PopoverMenu,
    Tooltip, prelude::*,
};
use workspace::Workspace;

use crate::conversation::{
    Entry, NoticeKind, PermissionRequest, PermissionStatus, ToolCall, ToolStatus, TurnState,
};
use crate::protocol::{PermissionDecision, PermissionMode};
use crate::session::{ClaudeSession, ProcessStatus, PromptImage, SessionEvent};
use crate::settings::{ClaudeCodeSettings, spinner_verbs};
use crate::{
    AcceptSuggestion, AllowPermission, AllowPermissionAlways, BlurInput, CycleMode, DenyPermission,
    FocusInput, FocusLastMessage, Interrupt, NewConversation, Send, ToggleFocusView,
    ToggleThinking,
};

const SPINNER_INTERVAL: Duration = Duration::from_millis(2500);
const MAX_OUTPUT_LINES: usize = 40;

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
    expanded: HashSet<usize>,
    pub focus_view: bool,
    attachments: Vec<Attachment>,
    editor_context: Option<EditorContext>,
    context_dismissed: bool,
    spinner_verbs: Vec<String>,
    spinner_index: usize,
    banner: Option<SharedString>,
    thinking_enabled: bool,
    follow_tail: bool,
    _spinner: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
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
            cx.subscribe(&input, |this, _, event: &EditorEvent, cx| {
                if matches!(event, EditorEvent::BufferEdited) {
                    this.refresh_placeholder_later(cx);
                }
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
            expanded: HashSet::new(),
            focus_view: settings.focus_view,
            attachments: Vec::new(),
            editor_context: None,
            context_dismissed: false,
            spinner_verbs: spinner_verbs(&settings.spinner_verbs),
            spinner_index: 0,
            banner: None,
            thinking_enabled: true,
            follow_tail: true,
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
            if cfg!(test) {
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

    pub fn title(&self, cx: &App) -> SharedString {
        let conversation = &self.session.read(cx).conversation;
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
                self.follow_tail = self.is_scrolled_to_bottom();
                if self.follow_tail {
                    self.scroll_handle.scroll_to_bottom();
                }
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

    fn accept_suggestion(
        &mut self,
        _: &AcceptSuggestion,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let suggestion = self.session.read(cx).conversation.prompt_suggestion.clone();
        let input_empty = self.input.read(cx).text(cx).is_empty();
        match suggestion {
            Some(suggestion) if input_empty => {
                self.input
                    .update(cx, |input, cx| input.set_text(suggestion, window, cx));
            }
            _ => cx.propagate(),
        }
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
        let mut modes = vec![
            PermissionMode::Default,
            PermissionMode::AcceptEdits,
            PermissionMode::Plan,
        ];
        let supports_auto = self
            .session
            .read(cx)
            .initialize
            .as_ref()
            .is_some_and(|init| init.current_permission_mode.as_deref() == Some("auto"))
            || self.session.read(cx).conversation.permission_mode == Some(PermissionMode::Auto);
        if supports_auto {
            modes.insert(0, PermissionMode::Auto);
        }
        if ClaudeCodeSettings::get_global(cx).allow_dangerously_skip_permissions {
            modes.push(PermissionMode::BypassPermissions);
        }
        modes
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

    fn render_entries(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let settings = ClaudeCodeSettings::get_global(cx).clone();
        let entries = self.session.read(cx).conversation.entries.clone();
        let mut elements = Vec::with_capacity(entries.len());
        let mut previous_day = None;
        for (index, entry) in entries.iter().enumerate() {
            if self.focus_view && entry.is_activity() {
                continue;
            }
            if let Entry::AssistantText(text) = entry
                && text.parent_tool_use_id.is_some()
            {
                continue;
            }
            if let Entry::ToolCall(call) = entry
                && call.parent_tool_use_id.is_some()
            {
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
                Entry::User(user) => self.render_user(index, user, timestamp, cx),
                Entry::AssistantText(text) => {
                    let markdown = self.markdown_for(index, &text.text, cx);
                    timeline_row(
                        index,
                        Color::Default,
                        false,
                        MarkdownElement::new(
                            markdown,
                            MarkdownStyle::themed(MarkdownFont::Agent, window, cx),
                        )
                        .into_any_element(),
                        timestamp,
                    )
                }
                Entry::Thinking(thinking) => self.render_thinking(index, thinking, cx),
                Entry::ToolCall(call) => self.render_tool_call(index, call, cx),
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
                ),
            };
            elements.push(element);
        }
        elements
    }

    fn render_user(
        &self,
        index: usize,
        user: &crate::conversation::UserEntry,
        timestamp: Option<String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors();
        let text = strip_context_preamble(&user.text);
        v_flex()
            .id(("user", index))
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
                    .child(Label::new(text.to_string()).size(LabelSize::Small))
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
        timeline_row(index, Color::Muted, false, content.into_any_element(), None)
    }

    fn toggle_expanded(&mut self, index: usize, cx: &mut Context<Self>) {
        if !self.expanded.remove(&index) {
            self.expanded.insert(index);
        }
        cx.notify();
    }

    fn render_tool_call(
        &self,
        index: usize,
        call: &ToolCall,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors();
        let expanded = self.expanded.contains(&index);
        let (dot, running) = match call.status {
            ToolStatus::Streaming | ToolStatus::Running => (Color::Accent, true),
            ToolStatus::Succeeded => (Color::Success, false),
            ToolStatus::Failed => (Color::Error, false),
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
                Label::new(call.summary())
                    .size(LabelSize::Small)
                    .color(Color::Muted)
                    .truncate(),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_expanded(index, cx)));
        let input_text = tool_input_text(call);
        let output_text = call
            .result
            .as_deref()
            .map(|result| truncate_lines(result, MAX_OUTPUT_LINES));
        let block = |label: &'static str, text: String| {
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
                        .font_buffer(cx)
                        .text_xs()
                        .child(text),
                )
        };
        let body = v_flex()
            .mt_1()
            .rounded_md()
            .border_1()
            .border_color(colors.border_variant)
            .bg(colors.editor_background)
            .child(block("IN", input_text))
            .when_some(output_text, |this, output| {
                this.child(div().h_px().bg(colors.border_variant))
                    .child(block("OUT", output))
            });
        let content = v_flex()
            .w_full()
            .min_w_0()
            .child(header)
            .when(expanded, |this| this.child(body));
        timeline_row(index, dot, running, content.into_any_element(), None)
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
            );
        }
        let question = tool.title.clone().unwrap_or_else(|| {
            format!(
                "Allow Claude to use {}?",
                display_tool_name(&tool.tool_name)
            )
        });
        let always_label = if is_edit_tool(&tool.tool_name) {
            "Yes, allow all edits this session"
        } else {
            "Yes, and don't ask again"
        };
        let has_suggestions = !tool.permission_suggestions.is_empty();
        let request_for_allow = request.clone();
        let request_for_always = request.clone();
        let request_for_deny = request.clone();
        let deny_input = self.deny_input.clone();
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
                    .gap_1()
                    .child(
                        Button::new(("allow", index), "1  Yes")
                            .full_width()
                            .style(ButtonStyle::Filled)
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.answer_permission(&request_for_allow, Answer::Allow, cx)
                            })),
                    )
                    .when(has_suggestions, |this| {
                        this.child(
                            Button::new(("allow-always", index), format!("2  {always_label}"))
                                .full_width()
                                .style(ButtonStyle::Subtle)
                                .label_size(LabelSize::Small)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.answer_permission(
                                        &request_for_always,
                                        Answer::AllowAlways,
                                        cx,
                                    )
                                })),
                        )
                    })
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new(
                                    ("deny", index),
                                    if has_suggestions { "3  No" } else { "2  No" },
                                )
                                .style(ButtonStyle::Subtle)
                                .label_size(LabelSize::Small)
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        let message = this.deny_input.read(cx).text(cx);
                                        this.deny_input
                                            .update(cx, |input, cx| input.clear(window, cx));
                                        this.answer_permission(
                                            &request_for_deny,
                                            Answer::Deny(message),
                                            cx,
                                        )
                                    },
                                )),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .px_1()
                                    .rounded_sm()
                                    .border_1()
                                    .border_color(colors.border_variant)
                                    .key_context("ClaudeCodeDenyInput")
                                    .child(deny_input),
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
                            .icon_size(IconSize::Small)
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
                            .icon_size(IconSize::Small)
                            .tooltip(move |_, cx| {
                                Tooltip::for_action("Past Conversations", &crate::ShowSessions, cx)
                            })
                            .on_click(|_, window, cx| {
                                window.dispatch_action(crate::ShowSessions.boxed_clone(), cx)
                            }),
                    )
                    .child(
                        IconButton::new("new-conversation", IconName::Plus)
                            .icon_size(IconSize::Small)
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
        if self
            .session
            .read(cx)
            .conversation
            .prompt_suggestion
            .is_some()
            && self.input.read(cx).text(cx).is_empty()
        {
            key_context.add("has_suggestion");
        }
        let send_button = if running && self.input.read(cx).text(cx).trim().is_empty() {
            IconButton::new("stop", IconName::Stop)
                .shape(IconButtonShape::Square)
                .style(ButtonStyle::Filled)
                .icon_color(Color::Error)
                .tooltip(move |_, cx| Tooltip::for_action("Stop", &Interrupt, cx))
                .on_click(|_, window, cx| window.dispatch_action(Interrupt.boxed_clone(), cx))
        } else {
            IconButton::new("send", IconName::ArrowUp)
                .shape(IconButtonShape::Square)
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
            .children(self.render_chips(cx))
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
                IconButton::new("attach", IconName::Plus).icon_size(IconSize::Small),
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
        let this = cx.weak_entity();
        let commands = self
            .session
            .read(cx)
            .initialize
            .as_ref()
            .map(|init| init.commands.clone())
            .unwrap_or_default();
        PopoverMenu::new("command-menu")
            .trigger_with_tooltip(
                IconButton::new("commands", IconName::Slash).icon_size(IconSize::Small),
                Tooltip::text("Commands"),
            )
            .anchor(gpui::Anchor::BottomLeft)
            .menu(move |window, cx| {
                let this = this.clone();
                let commands = commands.clone();
                Some(ContextMenu::build(window, cx, move |mut menu, _, _| {
                    menu = menu
                        .action("New conversation", NewConversation.boxed_clone())
                        .action("Toggle focus view", ToggleFocusView.boxed_clone())
                        .action("Toggle thinking", ToggleThinking.boxed_clone())
                        .action("Past conversations", crate::ShowSessions.boxed_clone())
                        .action("Show logs", crate::ShowLogs.boxed_clone());
                    if !commands.is_empty() {
                        menu = menu.separator().header("Slash commands");
                    }
                    for command in commands {
                        let this = this.clone();
                        let name = command.name.clone();
                        let mut entry = ContextMenuEntry::new(format!("/{}", command.name));
                        if !command.description.is_empty() {
                            let description = command.description.clone();
                            entry = entry
                                .documentation_aside(ui::DocumentationSide::Right, move |_| {
                                    Label::new(description.clone()).into_any_element()
                                });
                        }
                        menu.push_item(entry.handler(move |window, cx| {
                            this.update(cx, |this, cx| {
                                this.input.update(cx, |input, cx| {
                                    input.set_text(format!("/{name} "), window, cx);
                                });
                                window.focus(&this.input.focus_handle(cx), cx);
                            })
                            .ok();
                        }));
                    }
                    menu
                }))
            })
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

    fn render_empty_state(&self, cx: &App) -> AnyElement {
        let session = self.session.read(cx);
        let settings = ClaudeCodeSettings::get_global(cx);
        let mut content = v_flex()
            .items_center()
            .gap_2()
            .p_4()
            .child(
                Icon::new(IconName::Sparkle)
                    .size(IconSize::XLarge)
                    .color(Color::Accent),
            )
            .child(Headline::new("Claude Code").size(HeadlineSize::Small))
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

fn workspace_project(workspace: &Workspace) -> Entity<Project> {
    workspace.project().clone()
}

#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    Allow,
    AllowWithInput(Value),
    AllowAlways,
    Deny(String),
}

impl Focusable for ClaudeCodeView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ClaudeCodeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entries = self.render_entries(window, cx);
        let empty = entries.is_empty();
        let colors = cx.theme().colors();
        v_flex()
            .size_full()
            .key_context("ClaudeCode")
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
            .on_action(cx.listener(Self::accept_suggestion))
            .on_action(cx.listener(Self::allow_permission))
            .on_action(cx.listener(Self::allow_permission_always))
            .on_action(cx.listener(Self::deny_permission))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::new_conversation_requested))
            .child(self.render_header(cx))
            .child(
                div()
                    .id("claude-messages")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll_handle)
                    .child(if empty {
                        self.render_empty_state(cx)
                    } else {
                        v_flex().p_2().gap_2().children(entries).into_any_element()
                    }),
            )
            .children(self.render_spinner(cx))
            .child(self.render_input(window, cx))
    }
}

fn timeline_row(
    index: usize,
    dot: Color,
    animated: bool,
    content: AnyElement,
    timestamp: Option<String>,
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
        .child(div().pt_0p5().flex_none().child(dot_element))
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
}
