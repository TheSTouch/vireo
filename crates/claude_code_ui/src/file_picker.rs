use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use fuzzy::{StringMatch, StringMatchCandidate};
use gpui::{App, Context, DismissEvent, Entity, Task, WeakEntity, Window};
use picker::{Picker, PickerDelegate};
use project::Project;
use ui::{HighlightedLabel, ListItem, ListItemSpacing, prelude::*};

use crate::view::{Attachment, ClaudeCodeView};

const MAX_CANDIDATES: usize = 200_000;
const MAX_RESULTS: usize = 100;

/// "Mention file from this project…": a fuzzy list of the project's files and
/// folders that adds the chosen one as an @-mention.
pub type FilePicker = Picker<FilePickerDelegate>;

pub struct FilePickerDelegate {
    view: WeakEntity<ClaudeCodeView>,
    candidates: Arc<Vec<(PathBuf, String, bool)>>,
    matches: Vec<StringMatch>,
    selected_index: usize,
}

impl FilePickerDelegate {
    pub fn new(
        view: WeakEntity<ClaudeCodeView>,
        project: Entity<Project>,
        respect_git_ignore: bool,
        cx: &App,
    ) -> Self {
        let mut candidates = Vec::new();
        let project = project.read(cx);
        let multiple_roots = project.visible_worktrees(cx).count() > 1;
        'worktrees: for worktree in project.visible_worktrees(cx) {
            let worktree = worktree.read(cx);
            let root = worktree.abs_path();
            let root_name = worktree.root_name_str().to_string();
            for entry in worktree.entries(!respect_git_ignore, 0) {
                if entry.path.is_empty() {
                    continue;
                }
                let relative = entry.path.as_unix_str().to_string();
                let display = if multiple_roots {
                    format!("{root_name}/{relative}")
                } else {
                    relative
                };
                let display = if entry.is_dir() {
                    format!("{display}/")
                } else {
                    display
                };
                candidates.push((root.join(entry.path.as_std_path()), display, entry.is_dir()));
                if candidates.len() >= MAX_CANDIDATES {
                    break 'worktrees;
                }
            }
        }
        Self {
            view,
            candidates: Arc::new(candidates),
            matches: Vec::new(),
            selected_index: 0,
        }
    }
}

impl PickerDelegate for FilePickerDelegate {
    type ListItem = ListItem;

    fn name() -> &'static str {
        "Claude Code file mention"
    }

    fn match_count(&self) -> usize {
        self.matches.len()
    }

    fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn set_selected_index(&mut self, index: usize, _: &mut Window, cx: &mut Context<FilePicker>) {
        self.selected_index = index.min(self.matches.len().saturating_sub(1));
        cx.notify();
    }

    fn placeholder_text(&self, _: &mut Window, _: &mut App) -> Arc<str> {
        "Mention a file or folder…".into()
    }

    fn update_matches(
        &mut self,
        query: String,
        _: &mut Window,
        cx: &mut Context<FilePicker>,
    ) -> Task<()> {
        let candidates = self.candidates.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |picker, cx| {
            let matches = if query.is_empty() {
                candidates
                    .iter()
                    .enumerate()
                    .filter(|(_, (_, _, is_dir))| !is_dir)
                    .take(MAX_RESULTS)
                    .map(|(index, (_, display, _))| StringMatch {
                        candidate_id: index,
                        string: display.clone(),
                        positions: Vec::new(),
                        score: 0.0,
                    })
                    .collect()
            } else {
                let strings: Vec<StringMatchCandidate> = candidates
                    .iter()
                    .enumerate()
                    .map(|(index, (_, display, _))| StringMatchCandidate::new(index, display))
                    .collect();
                fuzzy::match_strings(
                    &strings,
                    &query,
                    false,
                    true,
                    MAX_RESULTS,
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

    fn confirm(&mut self, _: bool, window: &mut Window, cx: &mut Context<FilePicker>) {
        let Some(candidate) = self
            .matches
            .get(self.selected_index)
            .and_then(|found| self.candidates.get(found.candidate_id))
        else {
            return;
        };
        let path = candidate.0.clone();
        self.view
            .update(cx, |view, cx| {
                view.add_attachment(Attachment::Mention(path), cx);
                window.focus(&view.input_focus_handle(cx), cx);
            })
            .ok();
        cx.emit(DismissEvent);
    }

    fn dismissed(&mut self, _: &mut Window, cx: &mut Context<FilePicker>) {
        cx.emit(DismissEvent);
    }

    fn render_match(
        &self,
        index: usize,
        selected: bool,
        _: &mut Window,
        _: &mut Context<FilePicker>,
    ) -> Option<Self::ListItem> {
        let found = self.matches.get(index)?;
        let is_dir = self
            .candidates
            .get(found.candidate_id)
            .is_some_and(|candidate| candidate.2);
        Some(
            ListItem::new(index)
                .inset(true)
                .spacing(ListItemSpacing::Sparse)
                .toggle_state(selected)
                .start_slot(
                    Icon::new(if is_dir {
                        IconName::Folder
                    } else {
                        IconName::File
                    })
                    .size(IconSize::Small)
                    .color(Color::Muted),
                )
                .child(HighlightedLabel::new(
                    found.string.clone(),
                    found.positions.clone(),
                )),
        )
    }
}

impl FilePickerDelegate {
    pub fn into_picker(self, window: &mut Window, cx: &mut Context<FilePicker>) -> FilePicker {
        Picker::uniform_list(self, window, cx)
    }
}

pub fn new_file_picker(
    view: WeakEntity<ClaudeCodeView>,
    project: Entity<Project>,
    respect_git_ignore: bool,
    window: &mut Window,
    cx: &mut Context<FilePicker>,
) -> FilePicker {
    let delegate = FilePickerDelegate::new(view, project, respect_git_ignore, cx);
    let mut picker = delegate.into_picker(window, cx);
    picker.refresh(window, cx);
    picker
}
