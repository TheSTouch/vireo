use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow};
use buffer_diff::{BufferDiff, DiffHunkStatus};
use claude_ide::{DiffOutcome, DiffRequest, DiffReviewProvider};
use editor::{Anchor, Editor, EditorEvent, MultiBuffer};
use futures::channel::oneshot;
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, Global, Pixels,
    SharedString, Task, WeakEntity, Window, prelude::*,
};
use language::Buffer;
use serde_json::{Value, json};
use ui::{ContextMenu, Tooltip, prelude::*};
use workspace::Workspace;
use workspace::item::{Item, ItemEvent};

use crate::{AcceptProposedDiff, AcceptProposedHunk, RejectProposedDiff, RejectProposedHunk};

/// The contents a proposed Edit, MultiEdit or Write would leave in a file.
pub fn proposed_contents(tool_name: &str, input: &Value, current: &str) -> Result<String> {
    let string = |value: &Value, key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| anyhow!("missing {key}"))
    };
    let apply_edit = |text: &str, edit: &Value| -> Result<String> {
        let old = string(edit, "old_string")?;
        let new = string(edit, "new_string")?;
        let replace_all = edit
            .get("replace_all")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if old.is_empty() {
            return Ok(new);
        }
        if !text.contains(&old) {
            return Err(anyhow!("the text to replace is not in the file"));
        }
        Ok(if replace_all {
            text.replace(&old, &new)
        } else {
            text.replacen(&old, &new, 1)
        })
    };
    match tool_name {
        "Write" => string(input, "content"),
        "Edit" => apply_edit(current, input),
        "MultiEdit" => {
            let edits = input
                .get("edits")
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow!("missing edits"))?;
            edits
                .iter()
                .try_fold(current.to_string(), |text, edit| apply_edit(&text, edit))
        }
        other => Err(anyhow!("{other} does not propose file contents")),
    }
}

/// The tool input to allow once the person changed the proposal: the same
/// tool, rewritten so that it produces `final_contents`.
pub fn updated_input(tool_name: &str, input: &Value, current: &str, final_contents: &str) -> Value {
    let mut updated = input.clone();
    match tool_name {
        "Write" => updated["content"] = json!(final_contents),
        "Edit" if current.is_empty() => {
            updated["old_string"] = json!("");
            updated["new_string"] = json!(final_contents);
            updated["replace_all"] = json!(false);
        }
        "Edit" => {
            updated["old_string"] = json!(current);
            updated["new_string"] = json!(final_contents);
            updated["replace_all"] = json!(false);
        }
        "MultiEdit" => {
            updated["edits"] = json!([{"old_string": current, "new_string": final_contents, "replace_all": false}]);
        }
        _ => {}
    }
    updated
}

/// Applies one hunk to a text: replaces `range` of `text` with `replacement`.
pub fn splice(text: &str, range: Range<usize>, replacement: &str) -> String {
    let start = range.start.min(text.len());
    let end = range.end.clamp(start, text.len());
    format!("{}{replacement}{}", &text[..start], &text[end..])
}

#[derive(Default)]
struct ProposedDiffs(Vec<WeakEntity<ProposedDiff>>);

impl Global for ProposedDiffs {}

/// A proposed change to one file, shown as a diff that the person accepts or
/// rejects as a whole or hunk by hunk. The right side is editable.
pub struct ProposedDiff {
    pub tab_name: String,
    pub path: PathBuf,
    original: String,
    base_text: String,
    buffer: Entity<Buffer>,
    diff: Entity<BufferDiff>,
    editor: Entity<Editor>,
    resolver: Option<oneshot::Sender<DiffOutcome>>,
    pending_base: Option<Task<()>>,
    _subscription: gpui::Subscription,
}

impl EventEmitter<ItemEvent> for ProposedDiff {}

impl ProposedDiff {
    pub fn open(
        workspace: &Entity<Workspace>,
        request: DiffRequest,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(Entity<ProposedDiff>, oneshot::Receiver<DiffOutcome>)> {
        let original = match std::fs::read_to_string(&request.old_file_path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("failed to read {}", request.old_file_path.display())
                });
            }
        };
        let (sender, receiver) = oneshot::channel();
        let project = workspace.read(cx).project().clone();
        let language_registry = project.read(cx).languages().clone();
        let path = request.new_file_path.clone();
        let entity = cx.new(|cx| {
            let original = normalize(&original);
            let proposed = normalize(&request.new_file_contents);
            let buffer = cx.new(|cx| Buffer::local(proposed, cx));
            let snapshot = buffer.read(cx).text_snapshot();
            let diff = cx.new(|cx| BufferDiff::new(&snapshot, None, None, cx));
            let multibuffer = cx.new(|cx| {
                let mut multibuffer = MultiBuffer::singleton(buffer.clone(), cx);
                multibuffer.add_diff(diff.clone(), cx);
                multibuffer
            });
            let this = cx.weak_entity();
            let editor = cx.new(|cx| {
                let mut editor =
                    Editor::for_multibuffer(multibuffer, Some(project.clone()), window, cx);
                editor.set_expand_all_diff_hunks(cx);
                editor.set_diff_hunk_renderer(
                    Some(Arc::new(ProposedHunkRenderer { diff: this.clone() })),
                    cx,
                );
                editor.set_custom_context_menu(move |_, _, window, cx| {
                    Some(ContextMenu::build(window, cx, |menu, _, _| {
                        menu.action("Accept Change at Cursor", Box::new(AcceptProposedHunk))
                            .action("Reject Change at Cursor", Box::new(RejectProposedHunk))
                            .separator()
                            .action("Accept Proposed Changes", Box::new(AcceptProposedDiff))
                            .action("Reject Proposed Changes", Box::new(RejectProposedDiff))
                    }))
                });
                editor
            });
            let subscription =
                cx.subscribe(&editor, |this: &mut Self, _, event: &EditorEvent, cx| {
                    if matches!(event, EditorEvent::BufferEdited) {
                        this.refresh_diff(cx);
                    }
                });
            let mut this = Self {
                tab_name: request.tab_name.clone(),
                path: path.clone(),
                original: original.clone(),
                base_text: original,
                buffer: buffer.clone(),
                diff,
                editor,
                resolver: Some(sender),
                pending_base: None,
                _subscription: subscription,
            };
            this.refresh_diff(cx);
            let language_path = path.clone();
            let language_registry = language_registry.clone();
            cx.spawn(async move |_, cx| {
                let language = language_registry
                    .load_language_for_file_path(&language_path)
                    .await;
                if let Ok(language) = language {
                    buffer.update(cx, |buffer, cx| buffer.set_language(Some(language), cx));
                }
            })
            .detach();
            this
        });
        let registry = cx.default_global::<ProposedDiffs>();
        registry.0.retain(|diff| diff.upgrade().is_some());
        registry.0.push(entity.downgrade());
        workspace.update(cx, |workspace, cx| {
            workspace.add_item_to_center(Box::new(entity.clone()), window, cx);
        });
        Ok((entity, receiver))
    }

    fn refresh_diff(&mut self, cx: &mut Context<Self>) {
        let snapshot = self.buffer.read(cx).text_snapshot();
        let base: Arc<str> = self.base_text.clone().into();
        let task = self
            .diff
            .update(cx, |diff, cx| diff.set_base_text(Some(base), snapshot, cx));
        self.pending_base = Some(cx.spawn(async move |this, cx| {
            task.await;
            this.update(cx, |this, cx| this.resolve_if_reviewed(cx))
                .ok();
        }));
    }

    fn hunk_count(&self, cx: &App) -> usize {
        let snapshot = self.editor.read(cx).buffer().read(cx).snapshot(cx);
        snapshot.diff_hunks().count()
    }

    /// Once every hunk was accepted or rejected, the review is over.
    fn resolve_if_reviewed(&mut self, cx: &mut Context<Self>) {
        if self.resolver.is_none() || self.hunk_count(cx) > 0 {
            return;
        }
        let text = self.buffer.read(cx).text();
        if text == self.original {
            self.finish(DiffOutcome::Rejected, cx);
        } else {
            self.finish(DiffOutcome::Saved(text), cx);
        }
    }

    pub fn accept_all(&mut self, cx: &mut Context<Self>) {
        let text = self.buffer.read(cx).text();
        self.finish(DiffOutcome::Saved(text), cx);
    }

    pub fn reject_all(&mut self, cx: &mut Context<Self>) {
        self.finish(DiffOutcome::Rejected, cx);
    }

    fn finish(&mut self, outcome: DiffOutcome, cx: &mut Context<Self>) {
        if let Some(resolver) = self.resolver.take() {
            resolver.send(outcome).ok();
        }
        cx.emit(ItemEvent::CloseItem);
    }

    /// Closes the review without an answer, when the question was answered
    /// elsewhere (the permission card, or the CLI's `close_tab`).
    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        self.resolver = None;
        cx.emit(ItemEvent::CloseItem);
    }

    fn hunk_at(
        &self,
        range: Option<Range<Anchor>>,
        cx: &App,
    ) -> Option<(Range<usize>, Range<usize>)> {
        let editor = self.editor.read(cx);
        let snapshot = editor.buffer().read(cx).snapshot(cx);
        let range = range.unwrap_or_else(|| {
            let selection = editor.selections.newest_anchor();
            selection.start..selection.end
        });
        let hunk = editor.diff_hunks_in_ranges(&[range], &snapshot).next()?;
        let buffer_snapshot = self.buffer.read(cx).snapshot();
        let buffer_range = text::ToOffset::to_offset(&hunk.buffer_range.start, &buffer_snapshot)
            ..text::ToOffset::to_offset(&hunk.buffer_range.end, &buffer_snapshot);
        let base_range = hunk.diff_base_byte_range.start.0..hunk.diff_base_byte_range.end.0;
        Some((buffer_range, base_range))
    }

    pub fn accept_hunk(&mut self, range: Option<Range<Anchor>>, cx: &mut Context<Self>) {
        let Some((buffer_range, base_range)) = self.hunk_at(range, cx) else {
            return;
        };
        let replacement: String = self.buffer.read(cx).text_for_range(buffer_range).collect();
        self.base_text = splice(&self.base_text, base_range, &replacement);
        self.refresh_diff(cx);
    }

    pub fn reject_hunk(&mut self, range: Option<Range<Anchor>>, cx: &mut Context<Self>) {
        let Some((buffer_range, base_range)) = self.hunk_at(range, cx) else {
            return;
        };
        let start = base_range.start.min(self.base_text.len());
        let end = base_range.end.clamp(start, self.base_text.len());
        let original = self.base_text[start..end].to_string();
        self.buffer.update(cx, |buffer, cx| {
            buffer.edit([(buffer_range, original)], None, cx);
        });
    }
}

pub fn normalize(text: &str) -> String {
    let mut text = text.to_string();
    text::LineEnding::normalize(&mut text);
    text
}

impl Focusable for ProposedDiff {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.editor.focus_handle(cx)
    }
}

impl Render for ProposedDiff {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let file_name = self
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        v_flex()
            .size_full()
            .key_context("ClaudeCodeProposedDiff")
            .on_action(cx.listener(|this, _: &AcceptProposedDiff, _, cx| this.accept_all(cx)))
            .on_action(cx.listener(|this, _: &RejectProposedDiff, _, cx| this.reject_all(cx)))
            .on_action(
                cx.listener(|this, _: &AcceptProposedHunk, _, cx| this.accept_hunk(None, cx)),
            )
            .on_action(
                cx.listener(|this, _: &RejectProposedHunk, _, cx| this.reject_hunk(None, cx)),
            )
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .gap_2()
                    .justify_between()
                    .border_b_1()
                    .border_color(colors.border)
                    .bg(colors.toolbar_background)
                    .child(
                        h_flex()
                            .gap_1()
                            .min_w_0()
                            .child(
                                Icon::new(IconName::Sparkle)
                                    .size(IconSize::Small)
                                    .color(Color::Accent),
                            )
                            .child(
                                Label::new(format!("Claude proposes changes to {file_name}"))
                                    .size(LabelSize::Small),
                            )
                            .child(
                                Label::new(self.path.to_string_lossy().into_owned())
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted)
                                    .truncate(),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new("reject-all", "Reject")
                                    .label_size(LabelSize::Small)
                                    .tooltip(Tooltip::for_action_title(
                                        "Reject Proposed Changes",
                                        &RejectProposedDiff,
                                    ))
                                    .on_click(cx.listener(|this, _, _, cx| this.reject_all(cx))),
                            )
                            .child(
                                Button::new("accept-all", "Accept")
                                    .style(ButtonStyle::Filled)
                                    .label_size(LabelSize::Small)
                                    .tooltip(Tooltip::for_action_title(
                                        "Accept Proposed Changes",
                                        &AcceptProposedDiff,
                                    ))
                                    .on_click(cx.listener(|this, _, _, cx| this.accept_all(cx))),
                            ),
                    ),
            )
            .child(div().flex_1().min_h_0().child(self.editor.clone()))
    }
}

impl Item for ProposedDiff {
    type Event = ItemEvent;

    fn tab_content_text(&self, _: usize, _: &App) -> SharedString {
        let file_name = self
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        format!("{file_name} (proposed)").into()
    }

    fn tab_icon(&self, _: &Window, _: &App) -> Option<Icon> {
        Some(Icon::new(IconName::Sparkle).color(Color::Accent))
    }

    fn tab_tooltip_text(&self, _: &App) -> Option<SharedString> {
        Some(format!("Changes Claude proposes to {}", self.path.display()).into())
    }

    fn to_item_events(event: &Self::Event, f: &mut dyn FnMut(ItemEvent)) {
        f(*event)
    }
}

// Closing the tab without an answer rejects the proposal.
impl Drop for ProposedDiff {
    fn drop(&mut self) {
        if let Some(resolver) = self.resolver.take() {
            resolver.send(DiffOutcome::Rejected).ok();
        }
    }
}

struct ProposedHunkRenderer {
    diff: WeakEntity<ProposedDiff>,
}

impl editor::DiffHunkRenderer for ProposedHunkRenderer {
    fn render_hunk_controls(
        &self,
        row: u32,
        _status: &DiffHunkStatus,
        hunk_range: Range<Anchor>,
        _is_created_file: bool,
        line_height: Pixels,
        _editor: &Entity<Editor>,
        _window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let colors = cx.theme().colors();
        let accept_range = hunk_range.clone();
        let reject_range = hunk_range;
        let accept_diff = self.diff.clone();
        let reject_diff = self.diff.clone();
        h_flex()
            .h(line_height)
            .mr_1()
            .gap_1()
            .px_1()
            .border_x_1()
            .border_b_1()
            .border_color(colors.border)
            .rounded_b_md()
            .bg(colors.editor_background)
            .block_mouse_except_scroll()
            .child(
                Button::new(("reject-hunk", row as u64), "Reject this change")
                    .label_size(LabelSize::XSmall)
                    .on_click(move |_, _, cx| {
                        reject_diff
                            .update(cx, |diff, cx| {
                                diff.reject_hunk(Some(reject_range.clone()), cx)
                            })
                            .ok();
                    }),
            )
            .child(
                Button::new(("accept-hunk", row as u64), "Accept this change")
                    .label_size(LabelSize::XSmall)
                    .style(ButtonStyle::Filled)
                    .on_click(move |_, _, cx| {
                        accept_diff
                            .update(cx, |diff, cx| {
                                diff.accept_hunk(Some(accept_range.clone()), cx)
                            })
                            .ok();
                    }),
            )
            .into_any_element()
    }

    fn render_hunk_as_staged(&self, _: &DiffHunkStatus, _: &App) -> bool {
        false
    }
}

/// Serves `openDiff`, `close_tab` and `closeAllDiffTabs` for Vireo's IDE server
/// and opens the diffs of the panel's own permission prompts.
pub struct PanelDiffProvider;

impl DiffReviewProvider for PanelDiffProvider {
    fn open_diff(
        &self,
        workspace: Entity<Workspace>,
        request: DiffRequest,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<DiffOutcome>> {
        match ProposedDiff::open(&workspace, request, window, cx) {
            Ok((_, receiver)) => {
                cx.background_spawn(
                    async move { Ok(receiver.await.unwrap_or(DiffOutcome::Rejected)) },
                )
            }
            Err(error) => Task::ready(Err(error)),
        }
    }

    fn close_tab(&self, tab_name: &str, _: &mut Window, cx: &mut App) -> bool {
        close_matching(cx, |diff| diff.tab_name == tab_name) > 0
    }

    fn close_all(&self, _: &mut Window, cx: &mut App) -> usize {
        close_matching(cx, |_| true)
    }
}

pub fn close_matching(cx: &mut App, predicate: impl Fn(&ProposedDiff) -> bool) -> usize {
    let diffs: Vec<Entity<ProposedDiff>> = cx
        .try_global::<ProposedDiffs>()
        .map(|registry| registry.0.iter().filter_map(WeakEntity::upgrade).collect())
        .unwrap_or_default();
    let mut closed = 0;
    for diff in diffs {
        if predicate(diff.read(cx)) {
            diff.update(cx, |diff, cx| diff.dismiss(cx));
            closed += 1;
        }
    }
    closed
}

pub fn path_for_tool(input: &Value) -> Option<&Path> {
    input
        .get("file_path")
        .and_then(Value::as_str)
        .map(Path::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proposed_contents_for_each_edit_tool() {
        let current = "a\nb\nb\n";
        assert_eq!(
            proposed_contents(
                "Edit",
                &json!({"file_path": "/f", "old_string": "b", "new_string": "c"}),
                current
            )
            .unwrap(),
            "a\nc\nb\n"
        );
        assert_eq!(
            proposed_contents(
                "Edit",
                &json!({"old_string": "b", "new_string": "c", "replace_all": true}),
                current
            )
            .unwrap(),
            "a\nc\nc\n"
        );
        assert_eq!(
            proposed_contents("MultiEdit", &json!({"edits": [{"old_string": "a", "new_string": "x"}, {"old_string": "x\nb", "new_string": "y"}]}), current).unwrap(),
            "y\nb\n"
        );
        assert_eq!(
            proposed_contents("Write", &json!({"content": "new"}), current).unwrap(),
            "new"
        );
        assert!(
            proposed_contents(
                "Edit",
                &json!({"old_string": "zzz", "new_string": "c"}),
                current
            )
            .is_err()
        );
        assert!(proposed_contents("Bash", &json!({}), current).is_err());
    }

    #[test]
    fn updated_input_reproduces_the_final_contents() {
        let current = "a\nb\n";
        let edit = json!({"file_path": "/f", "old_string": "b", "new_string": "c"});
        let updated = updated_input("Edit", &edit, current, "a\nz\n");
        assert_eq!(
            proposed_contents("Edit", &updated, current).unwrap(),
            "a\nz\n"
        );
        assert_eq!(updated["file_path"], "/f");

        let multi = json!({"file_path": "/f", "edits": [{"old_string": "a", "new_string": "q"}]});
        let updated = updated_input("MultiEdit", &multi, current, "q\nb\nmore\n");
        assert_eq!(
            proposed_contents("MultiEdit", &updated, current).unwrap(),
            "q\nb\nmore\n"
        );

        let write = json!({"file_path": "/f", "content": "x"});
        assert_eq!(updated_input("Write", &write, current, "y")["content"], "y");
    }

    #[gpui::test]
    async fn reviewing_hunk_by_hunk_saves_the_reviewed_text(cx: &mut gpui::TestAppContext) {
        let app_state = cx.update(|cx| {
            let state = workspace::AppState::test(cx);
            theme_settings::init(theme::LoadThemes::JustBase, cx);
            editor::init(cx);
            <crate::settings::ClaudeCodeSettings as settings::Settings>::register(cx);
            state
        });
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.txt");
        std::fs::write(&path, "a\nb\nc\nd\ne\nf\ng\nh\n").unwrap();
        let project = project::Project::test(app_state.fs.clone(), [], cx).await;
        let (multi_workspace, cx) = cx
            .add_window_view(|window, cx| workspace::MultiWorkspace::test_new(project, window, cx));
        let workspace =
            multi_workspace.read_with(cx, |multi_workspace, _| multi_workspace.workspace().clone());
        let (diff, receiver) = cx.update(|window, cx| {
            ProposedDiff::open(
                &workspace,
                DiffRequest {
                    old_file_path: path.clone(),
                    new_file_path: path.clone(),
                    new_file_contents: "a\nB\nc\nd\ne\nf\nG\nh\n".into(),
                    tab_name: "tab".into(),
                },
                window,
                cx,
            )
            .unwrap()
        });
        cx.run_until_parked();
        let anchor_at_row = |diff: &ProposedDiff, row: u32, cx: &App| {
            let snapshot = diff.editor.read(cx).buffer().read(cx).snapshot(cx);
            let anchor = snapshot.anchor_before(language::Point::new(row, 0));
            anchor..anchor
        };
        diff.update(cx, |diff, cx| {
            assert_eq!(diff.hunk_count(cx), 2);
            let range = anchor_at_row(diff, 1, cx);
            diff.accept_hunk(Some(range), cx);
        });
        cx.run_until_parked();
        diff.update(cx, |diff, cx| {
            assert_eq!(diff.hunk_count(cx), 1);
            let range = anchor_at_row(diff, 6, cx);
            diff.reject_hunk(Some(range), cx);
        });
        cx.run_until_parked();
        match receiver.await.unwrap() {
            DiffOutcome::Saved(text) => assert_eq!(text, "a\nB\nc\nd\ne\nf\ng\nh\n"),
            DiffOutcome::Rejected => panic!("expected the reviewed text"),
        }
    }

    #[test]
    fn splicing_hunks() {
        assert_eq!(splice("hello world", 6..11, "there"), "hello there");
        assert_eq!(splice("abc", 1..1, "X"), "aXbc");
        assert_eq!(splice("abc", 2..10, ""), "ab");
    }
}
