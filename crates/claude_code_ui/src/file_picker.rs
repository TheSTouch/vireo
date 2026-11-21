use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use fuzzy::{StringMatch, StringMatchCandidate};
use gpui::{App, BackgroundExecutor, Context, DismissEvent, Entity, Task, WeakEntity, Window};
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
    candidates: Arc<Vec<ProjectEntry>>,
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
        Self {
            view,
            candidates: Arc::new(project_entries(project.read(cx), respect_git_ignore, cx)),
            matches: Vec::new(),
            selected_index: 0,
        }
    }
}

/// A file or folder of the project that can be mentioned.
pub struct ProjectEntry {
    pub abs_path: PathBuf,
    /// The path shown in lists: relative to its root, prefixed with the
    /// root's name when the project has several, and ending in "/" for folders.
    pub display: String,
    pub is_dir: bool,
}

pub fn project_entries(project: &Project, respect_git_ignore: bool, cx: &App) -> Vec<ProjectEntry> {
    let mut candidates = Vec::new();
    let multiple_roots = project.visible_worktrees(cx).count() > 1;
    for worktree in project.visible_worktrees(cx) {
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
            candidates.push(ProjectEntry {
                abs_path: root.join(entry.path.as_std_path()),
                display,
                is_dir: entry.is_dir(),
            });
            if candidates.len() >= MAX_CANDIDATES {
                return candidates;
            }
        }
    }
    candidates
}

/// The entries matching `query`, best first. With no query, the first files.
pub async fn match_entries(
    candidates: &[ProjectEntry],
    query: &str,
    max_results: usize,
    executor: BackgroundExecutor,
) -> Vec<StringMatch> {
    if query.is_empty() {
        return candidates
            .iter()
            .enumerate()
            .filter(|(_, entry)| !entry.is_dir)
            .take(max_results)
            .map(|(index, entry)| StringMatch {
                candidate_id: index,
                string: entry.display.clone(),
                positions: Vec::new(),
                score: 0.0,
            })
            .collect();
    }
    let strings: Vec<StringMatchCandidate> = candidates
        .iter()
        .enumerate()
        .map(|(index, entry)| StringMatchCandidate::new(index, &entry.display))
        .collect();
    fuzzy::match_strings(
        &strings,
        query,
        false,
        true,
        max_results,
        &AtomicBool::new(false),
        executor,
    )
    .await
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
            let matches = match_entries(&candidates, &query, MAX_RESULTS, executor).await;
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
        let path = candidate.abs_path.clone();
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
            .is_some_and(|candidate| candidate.is_dir);
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
