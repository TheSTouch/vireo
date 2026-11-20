pub mod server;

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use anyhow::Result;
use collections::HashMap;
use editor::scroll::Autoscroll;
use editor::{Editor, EditorEvent, SelectionEffects};
use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui::{
    AnyWindowHandle, App, AppContext as _, Context, Entity, EntityId, Global, Subscription, Task,
    WeakEntity, Window,
};
use language::{Point, PointUtf16};
use lsp::DiagnosticSeverity;
use project::Project;
use serde_json::{Value, json};
use util::ResultExt as _;
use workspace::{OpenOptions, Workspace};

pub use crate::server::{
    IdeServer, LockFile, Route, SDK_TOOLS, ToolCallRequest, ToolResult, error_response,
    route_message, route_sdk_message, tool_definitions, tool_response,
};

const SELECTION_DEBOUNCE: Duration = Duration::from_millis(50);

/// A diff of proposed changes that Claude asked the person to review, through
/// `openDiff` or a permission prompt.
#[derive(Clone, Debug, PartialEq)]
pub struct DiffRequest {
    pub old_file_path: PathBuf,
    pub new_file_path: PathBuf,
    pub new_file_contents: String,
    pub tab_name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DiffOutcome {
    /// Accepted, possibly only in part. Carries the final contents.
    Saved(String),
    Rejected,
}

/// Opens and closes proposed-diff views. The Claude Code panel provides it;
/// without one, `openDiff` reports that diffs cannot be shown.
pub trait DiffReviewProvider: 'static {
    fn open_diff(
        &self,
        workspace: Entity<Workspace>,
        request: DiffRequest,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<DiffOutcome>>;
    fn close_tab(&self, tab_name: &str, window: &mut Window, cx: &mut App) -> bool;
    fn close_all(&self, window: &mut Window, cx: &mut App) -> usize;
}

#[derive(Clone)]
pub struct GlobalDiffReviewProvider(pub Rc<dyn DiffReviewProvider>);

impl Global for GlobalDiffReviewProvider {}

/// Runs code for the `executeCode` tool.
pub trait CodeExecutor: 'static {
    fn execute(
        &self,
        workspace: Entity<Workspace>,
        code: String,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<ToolResult>>;
}

#[derive(Clone)]
pub struct GlobalCodeExecutor(pub Rc<dyn CodeExecutor>);

impl Global for GlobalCodeExecutor {}

#[derive(Default)]
struct ClaudeIdes(HashMap<EntityId, Entity<ClaudeIde>>);

impl Global for ClaudeIdes {}

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, window, cx| {
        let Some(window) = window else {
            return;
        };
        if !workspace.project().read(cx).is_local() {
            return;
        }
        let workspace_id = cx.entity_id();
        let window_handle = window.window_handle();
        let workspace_handle = cx.entity().downgrade();
        // The workspace is still being built here, so it cannot be read:
        // hand its project over directly.
        let project = workspace.project().clone();
        let ide = cx.new(|cx| ClaudeIde::new(workspace_handle, project, window_handle, cx));
        cx.default_global::<ClaudeIdes>()
            .0
            .insert(workspace_id, ide);
        cx.on_release(move |_, cx| {
            if let Some(ide) = cx.default_global::<ClaudeIdes>().0.remove(&workspace_id) {
                ide.update(cx, |ide, cx| ide.shut_down(cx));
            }
        })
        .detach();
    })
    .detach();
}

#[derive(Clone, Debug, PartialEq)]
pub struct SelectionInfo {
    pub file_path: PathBuf,
    pub text: String,
    pub start: PointUtf16,
    pub end: PointUtf16,
    /// Rows of the selection in the buffer, 0-based, inclusive.
    pub rows: (u32, u32),
}

impl SelectionInfo {
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    pub fn to_notification(&self) -> Value {
        json!({
            "text": self.text,
            "filePath": self.file_path.to_string_lossy(),
            "fileUrl": file_url(&self.file_path),
            "selection": {
                "start": {"line": self.start.row, "character": self.start.column},
                "end": {"line": self.end.row, "character": self.end.column},
                "isEmpty": self.is_empty(),
            }
        })
    }

    fn to_tool_json(&self) -> Value {
        json!({
            "success": true,
            "text": self.text,
            "filePath": self.file_path.to_string_lossy(),
            "selection": {
                "start": {"line": self.start.row, "character": self.start.column},
                "end": {"line": self.end.row, "character": self.end.column},
                "isEmpty": self.is_empty(),
            }
        })
    }
}

pub fn file_url(path: &Path) -> String {
    url::Url::from_file_path(path)
        .map(|url| url.to_string())
        .unwrap_or_else(|_| format!("file://{}", path.to_string_lossy()))
}

fn path_from_uri(uri: &str) -> PathBuf {
    url::Url::parse(uri)
        .ok()
        .filter(|url| url.scheme() == "file")
        .and_then(|url| url.to_file_path().ok())
        .unwrap_or_else(|| PathBuf::from(uri))
}

/// The selection of an editor that shows one file on disk.
pub fn editor_selection(editor: &Editor, cx: &App) -> Option<SelectionInfo> {
    let multibuffer = editor.buffer().read(cx);
    let buffer = multibuffer.as_singleton()?;
    let buffer = buffer.read(cx);
    let file_path = buffer.file()?.as_local()?.abs_path(cx);
    let snapshot = multibuffer.snapshot(cx);
    let selection = editor.selections.newest_anchor();
    let (start, buffer_snapshot) = snapshot.anchor_to_buffer_anchor(selection.start)?;
    let (end, _) = snapshot.anchor_to_buffer_anchor(selection.end)?;
    let start: Point = text::ToPoint::to_point(&start, buffer_snapshot);
    let end: Point = text::ToPoint::to_point(&end, buffer_snapshot);
    let (start, end) = if start <= end {
        (start, end)
    } else {
        (end, start)
    };
    Some(SelectionInfo {
        file_path,
        text: buffer_snapshot.text_for_range(start..end).collect(),
        start: buffer_snapshot.point_to_point_utf16(start),
        end: buffer_snapshot.point_to_point_utf16(end),
        rows: (start.row, end.row),
    })
}

/// Vireo's side of the Claude Code IDE integration for one workspace window.
pub struct ClaudeIde {
    workspace: WeakEntity<Workspace>,
    window: AnyWindowHandle,
    server: Option<IdeServer>,
    lock_path: Option<PathBuf>,
    latest_selection: Option<SelectionInfo>,
    active_editor: Option<WeakEntity<Editor>>,
    editor_subscription: Option<Subscription>,
    pending_selection: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
    _tasks: Vec<Task<()>>,
}

impl ClaudeIde {
    fn new(
        workspace: WeakEntity<Workspace>,
        project: Entity<Project>,
        window: AnyWindowHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            workspace: workspace.clone(),
            window,
            server: None,
            lock_path: None,
            latest_selection: None,
            active_editor: None,
            editor_subscription: None,
            pending_selection: None,
            _subscriptions: Vec::new(),
            _tasks: Vec::new(),
        };
        if let Some(workspace) = workspace.upgrade() {
            this._subscriptions.push(cx.subscribe(
                &workspace,
                |this, workspace, event: &workspace::Event, cx| {
                    if matches!(event, workspace::Event::ActiveItemChanged) {
                        this.track_active_editor(workspace, cx);
                    }
                },
            ));
            this._subscriptions
                .push(cx.subscribe(&project, |this, _, event, cx| {
                    if matches!(
                        event,
                        project::Event::WorktreeAdded(_) | project::Event::WorktreeRemoved(_)
                    ) {
                        this.write_lock_file(cx);
                    }
                }));
        }
        this.start_server(cx);
        this
    }

    pub fn for_workspace(workspace: &Entity<Workspace>, cx: &App) -> Option<Entity<ClaudeIde>> {
        cx.try_global::<ClaudeIdes>()?
            .0
            .get(&workspace.entity_id())
            .cloned()
    }

    pub fn port(&self) -> Option<u16> {
        self.server.as_ref().map(|server| server.port)
    }

    pub fn latest_selection(&self) -> Option<&SelectionInfo> {
        self.latest_selection.as_ref()
    }

    pub fn has_connected_clients(&self) -> bool {
        self.server
            .as_ref()
            .is_some_and(|server| server.client_count() > 0)
    }

    fn start_server(&mut self, cx: &mut Context<Self>) {
        let (tool_tx, mut tool_rx) = mpsc::unbounded::<ToolCallRequest>();
        let executor = cx.background_executor().clone();
        let version = release_channel::AppVersion::global(cx).to_string();
        let start = cx.background_spawn(IdeServer::start(executor, tool_tx, version));
        self._tasks.push(cx.spawn(async move |this, cx| {
            let server = match start.await {
                Ok(server) => server,
                Err(error) => {
                    log::error!("Claude Code IDE server failed to start: {error:#}");
                    return;
                }
            };
            let started = this.update(cx, |this, cx| {
                this.server = Some(server);
                this.write_lock_file(cx);
                this.set_terminal_env(cx);
            });
            if started.is_err() {
                return;
            }
            while let Some(call) = tool_rx.next().await {
                let Ok(task) = this.update(cx, |this, cx| {
                    this.call_tool(&call.name, call.arguments.clone(), cx)
                }) else {
                    break;
                };
                cx.background_spawn(async move {
                    call.reply.send(task.await).ok();
                })
                .detach();
            }
        }));
    }

    fn workspace_folders(&self, cx: &App) -> Vec<String> {
        let Some(workspace) = self.workspace.upgrade() else {
            return Vec::new();
        };
        workspace
            .read(cx)
            .visible_worktrees(cx)
            .map(|worktree| worktree.read(cx).abs_path().to_string_lossy().into_owned())
            .collect()
    }

    fn write_lock_file(&mut self, cx: &mut Context<Self>) {
        let Some(server) = &self.server else {
            return;
        };
        let lock = LockFile::new(self.workspace_folders(cx), server.auth_token.clone());
        let ide_dir = claude_config_dir().join("ide");
        let port = server.port;
        if let Some(path) = lock.write(&ide_dir, port).log_err() {
            self.lock_path = Some(path);
        }
    }

    fn set_terminal_env(&self, cx: &mut Context<Self>) {
        let (Some(workspace), Some(port)) = (self.workspace.upgrade(), self.port()) else {
            return;
        };
        let env = HashMap::from_iter([
            ("CLAUDE_CODE_SSE_PORT".to_string(), port.to_string()),
            ("ENABLE_IDE_INTEGRATION".to_string(), "true".to_string()),
        ]);
        workspace
            .read(cx)
            .project()
            .clone()
            .update(cx, |project, cx| project.set_extra_terminal_env(env, cx));
    }

    fn shut_down(&mut self, _cx: &mut Context<Self>) {
        self.server = None;
        if let Some(path) = self.lock_path.take()
            && let Err(error) = std::fs::remove_file(&path)
        {
            log::debug!("failed to remove {}: {error}", path.display());
        }
    }

    fn track_active_editor(&mut self, workspace: Entity<Workspace>, cx: &mut Context<Self>) {
        let editor = workspace
            .read(cx)
            .active_item(cx)
            .and_then(|item| item.act_as::<Editor>(cx));
        let Some(editor) = editor else {
            return;
        };
        if self
            .active_editor
            .as_ref()
            .is_some_and(|active| active.entity_id() == editor.entity_id())
        {
            return;
        }
        self.active_editor = Some(editor.downgrade());
        self.editor_subscription = Some(cx.subscribe(&editor, |this, editor, event, cx| {
            if matches!(event, EditorEvent::SelectionsChanged { .. }) {
                this.schedule_selection_update(editor, cx);
            }
        }));
        self.schedule_selection_update(editor, cx);
    }

    fn schedule_selection_update(&mut self, editor: Entity<Editor>, cx: &mut Context<Self>) {
        self.pending_selection = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SELECTION_DEBOUNCE).await;
            this.update(cx, |this, cx| {
                let Some(selection) = editor_selection(editor.read(cx), cx) else {
                    return;
                };
                if this.latest_selection.as_ref() == Some(&selection) {
                    return;
                }
                if let Some(server) = &this.server {
                    server.broadcast("selection_changed", selection.to_notification());
                }
                this.latest_selection = Some(selection);
            })
            .ok();
        }));
    }

    /// Sends an `at_mentioned` notification, which the CLI turns into an
    /// @-mention of the file and rows in its prompt.
    pub fn send_at_mention(&self, path: &Path, rows: Option<(u32, u32)>) -> bool {
        let Some(server) = &self.server else {
            return false;
        };
        if server.client_count() == 0 {
            return false;
        }
        let mut params = json!({"filePath": path.to_string_lossy()});
        if let Some((start, end)) = rows {
            params["lineStart"] = json!(start);
            params["lineEnd"] = json!(end);
        }
        server.broadcast("at_mentioned", params);
        true
    }

    pub fn call_tool(
        &mut self,
        name: &str,
        arguments: Value,
        cx: &mut Context<Self>,
    ) -> Task<ToolResult> {
        let Some(workspace) = self.workspace.upgrade() else {
            return Task::ready(ToolResult::error("The Vireo window is closed"));
        };
        let string = |key: &str| {
            arguments
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        match name {
            "getDiagnostics" => self.get_diagnostics(workspace, string("uri"), cx),
            "openDiff" => {
                let (Some(old), Some(new), Some(contents), Some(tab_name)) = (
                    string("old_file_path"),
                    string("new_file_path"),
                    string("new_file_contents"),
                    string("tab_name"),
                ) else {
                    return Task::ready(ToolResult::error(
                        "openDiff needs old_file_path, new_file_path, new_file_contents and tab_name",
                    ));
                };
                self.open_diff(
                    workspace,
                    DiffRequest {
                        old_file_path: PathBuf::from(old),
                        new_file_path: PathBuf::from(new),
                        new_file_contents: contents,
                        tab_name,
                    },
                    cx,
                )
            }
            "close_tab" => {
                let tab_name = string("tab_name").unwrap_or_default();
                self.with_diff_provider(cx, |provider, window, cx| {
                    provider.close_tab(&tab_name, window, cx);
                });
                Task::ready(ToolResult::text("TAB_CLOSED"))
            }
            "closeAllDiffTabs" => {
                let closed = self
                    .with_diff_provider(cx, |provider, window, cx| provider.close_all(window, cx))
                    .unwrap_or(0);
                Task::ready(ToolResult::text(format!("CLOSED_{closed}_DIFF_TABS")))
            }
            "executeCode" => {
                let Some(code) = string("code") else {
                    return Task::ready(ToolResult::error("executeCode needs code"));
                };
                let Some(executor) = cx.try_global::<GlobalCodeExecutor>().cloned() else {
                    return Task::ready(ToolResult::error(
                        "No Jupyter kernel is available in Vireo for this file.",
                    ));
                };
                let task = self.window.update(cx, |_, window, cx| {
                    executor.0.execute(workspace, code, window, cx)
                });
                match task {
                    Ok(task) => cx.background_spawn(async move {
                        task.await
                            .unwrap_or_else(|error| ToolResult::error(format!("{error:#}")))
                    }),
                    Err(error) => Task::ready(ToolResult::error(format!("{error:#}"))),
                }
            }
            "openFile" => self.open_file(workspace, &arguments, cx),
            "getCurrentSelection" => {
                let selection = workspace
                    .read(cx)
                    .active_item(cx)
                    .and_then(|item| item.act_as::<Editor>(cx))
                    .and_then(|editor| editor_selection(editor.read(cx), cx));
                Task::ready(match selection {
                    Some(selection) => ToolResult::json(&selection.to_tool_json()),
                    None => ToolResult::json(
                        &json!({"success": false, "message": "No active editor found"}),
                    ),
                })
            }
            "getLatestSelection" => Task::ready(match &self.latest_selection {
                Some(selection) => ToolResult::json(&selection.to_tool_json()),
                None => ToolResult::json(
                    &json!({"success": false, "message": "No selection available"}),
                ),
            }),
            "getOpenEditors" => Task::ready(ToolResult::json(&open_editors(&workspace, cx))),
            "getWorkspaceFolders" => {
                let folders: Vec<Value> = workspace
                    .read(cx)
                    .visible_worktrees(cx)
                    .map(|worktree| {
                        let worktree = worktree.read(cx);
                        let path = worktree.abs_path();
                        json!({
                            "name": worktree.root_name_str(),
                            "uri": file_url(&path),
                            "path": path.to_string_lossy(),
                        })
                    })
                    .collect();
                let root_path = folders
                    .first()
                    .and_then(|folder| folder.get("path").cloned());
                Task::ready(ToolResult::json(&json!({
                    "success": true,
                    "folders": folders,
                    "rootPath": root_path,
                })))
            }
            "checkDocumentDirty" => {
                let path = PathBuf::from(string("filePath").unwrap_or_default());
                let project = workspace.read(cx).project().clone();
                Task::ready(ToolResult::json(
                    &match open_buffer_for_path(&project, &path, cx) {
                        Some(buffer) => json!({
                            "success": true,
                            "filePath": path.to_string_lossy(),
                            "isDirty": buffer.read(cx).is_dirty(),
                            "isUntitled": false,
                        }),
                        None => {
                            json!({"success": false, "message": format!("Document not open: {}", path.display())})
                        }
                    },
                ))
            }
            "saveDocument" => {
                let path = PathBuf::from(string("filePath").unwrap_or_default());
                let project = workspace.read(cx).project().clone();
                let Some(buffer) = open_buffer_for_path(&project, &path, cx) else {
                    return Task::ready(ToolResult::json(&json!({
                        "success": false,
                        "message": format!("Document not open: {}", path.display()),
                    })));
                };
                let save = project.update(cx, |project, cx| project.save_buffer(buffer, cx));
                cx.background_spawn(async move {
                    match save.await {
                        Ok(()) => ToolResult::json(&json!({
                            "success": true,
                            "filePath": path.to_string_lossy(),
                            "saved": true,
                            "message": "Document saved successfully",
                        })),
                        Err(error) => ToolResult::json(&json!({
                            "success": false,
                            "message": format!("Failed to save {}: {error:#}", path.display()),
                        })),
                    }
                })
            }
            other => Task::ready(ToolResult::error(format!("Unknown tool: {other}"))),
        }
    }

    fn with_diff_provider<R>(
        &self,
        cx: &mut App,
        f: impl FnOnce(&dyn DiffReviewProvider, &mut Window, &mut App) -> R,
    ) -> Option<R> {
        let provider = cx.try_global::<GlobalDiffReviewProvider>()?.clone();
        self.window
            .update(cx, |_, window, cx| f(provider.0.as_ref(), window, cx))
            .log_err()
    }

    fn open_diff(
        &mut self,
        workspace: Entity<Workspace>,
        request: DiffRequest,
        cx: &mut Context<Self>,
    ) -> Task<ToolResult> {
        let tab_name = request.tab_name.clone();
        let Some(task) = self.with_diff_provider(cx, |provider, window, cx| {
            provider.open_diff(workspace, request, window, cx)
        }) else {
            return Task::ready(ToolResult::error("Vireo cannot show diffs in this window"));
        };
        cx.background_spawn(async move {
            match task.await {
                Ok(DiffOutcome::Saved(contents)) => {
                    ToolResult::texts(["FILE_SAVED".to_string(), contents])
                }
                Ok(DiffOutcome::Rejected) => {
                    ToolResult::texts(["DIFF_REJECTED".to_string(), tab_name])
                }
                Err(error) => ToolResult::error(format!("{error:#}")),
            }
        })
    }

    fn open_file(
        &mut self,
        workspace: Entity<Workspace>,
        arguments: &Value,
        cx: &mut Context<Self>,
    ) -> Task<ToolResult> {
        let Some(path) = arguments.get("filePath").and_then(Value::as_str) else {
            return Task::ready(ToolResult::error("openFile needs filePath"));
        };
        let path = path_from_uri(path);
        let start_text = arguments
            .get("startText")
            .and_then(Value::as_str)
            .map(str::to_string);
        let end_text = arguments
            .get("endText")
            .and_then(Value::as_str)
            .map(str::to_string);
        let select_to_end_of_line = arguments
            .get("selectToEndOfLine")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let make_frontmost = arguments
            .get("makeFrontmost")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let open = self.window.update(cx, |_, window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.open_abs_path(
                    path.clone(),
                    OpenOptions {
                        focus: Some(make_frontmost),
                        ..Default::default()
                    },
                    window,
                    cx,
                )
            })
        });
        let open = match open {
            Ok(open) => open,
            Err(error) => return Task::ready(ToolResult::error(format!("{error:#}"))),
        };
        let window = self.window;
        cx.spawn(async move |_, cx| {
            let item = match open.await {
                Ok(item) => item,
                Err(error) => {
                    return ToolResult::error(format!(
                        "Failed to open {}: {error:#}",
                        path.display()
                    ));
                }
            };
            let editor = cx.update(|cx| item.act_as::<Editor>(cx));
            let mut line_count = 0;
            if let Some(editor) = editor {
                window
                    .update(cx, |_, window, cx| {
                        editor.update(cx, |editor, cx| {
                            let text = editor.buffer().read(cx).snapshot(cx).text();
                            line_count = text.lines().count();
                            if let Some(range) = find_text_range(
                                &text,
                                start_text.as_deref(),
                                end_text.as_deref(),
                                select_to_end_of_line,
                            ) {
                                editor.change_selections(
                                    SelectionEffects::scroll(Autoscroll::center()),
                                    window,
                                    cx,
                                    |selections| selections.select_ranges([range]),
                                );
                            }
                        })
                    })
                    .log_err();
            }
            if make_frontmost {
                ToolResult::text(format!("Opened file: {}", path.display()))
            } else {
                let language = path
                    .extension()
                    .map(|extension| extension.to_string_lossy().into_owned())
                    .unwrap_or_default();
                ToolResult::json(&json!({
                    "success": true,
                    "filePath": path.to_string_lossy(),
                    "languageId": language,
                    "lineCount": line_count,
                }))
            }
        })
    }

    fn get_diagnostics(
        &mut self,
        workspace: Entity<Workspace>,
        uri: Option<String>,
        cx: &mut Context<Self>,
    ) -> Task<ToolResult> {
        let project = workspace.read(cx).project().clone();
        let paths: Vec<project::ProjectPath> = match &uri {
            Some(uri) => {
                let path = path_from_uri(uri);
                match project.read(cx).find_project_path(&path, cx) {
                    Some(project_path) => vec![project_path],
                    None => return Task::ready(ToolResult::json(&json!([]))),
                }
            }
            None => {
                let mut paths: Vec<project::ProjectPath> = project
                    .read(cx)
                    .diagnostic_summaries(false, cx)
                    .map(|(path, _, _)| path)
                    .collect();
                paths.dedup();
                paths
            }
        };
        let buffers: Vec<_> = paths
            .into_iter()
            .map(|path| project.update(cx, |project, cx| project.open_buffer(path, cx)))
            .collect();
        cx.spawn(async move |_, cx| {
            let mut files = Vec::new();
            for buffer in buffers {
                let Ok(buffer) = buffer.await else {
                    continue;
                };
                let file = cx.update(|cx| diagnostics_json(&buffer.read(cx).snapshot(), cx));
                if let Some(file) = file {
                    files.push(file);
                }
            }
            ToolResult::json(&Value::Array(files))
        })
    }
}

fn diagnostics_json(snapshot: &language::BufferSnapshot, cx: &App) -> Option<Value> {
    let path = snapshot.file()?.as_local()?.abs_path(cx);
    let diagnostics: Vec<Value> = snapshot
        .diagnostics_in_range::<_, PointUtf16>(0..snapshot.len(), false)
        .map(|entry| {
            let diagnostic = &entry.diagnostic;
            let severity = match diagnostic.severity {
                DiagnosticSeverity::ERROR => "Error",
                DiagnosticSeverity::WARNING => "Warning",
                DiagnosticSeverity::INFORMATION => "Information",
                _ => "Hint",
            };
            let mut value = json!({
                "message": diagnostic.message,
                "severity": severity,
                "range": {
                    "start": {"line": entry.range.start.row, "character": entry.range.start.column},
                    "end": {"line": entry.range.end.row, "character": entry.range.end.column},
                },
            });
            if let Some(source) = &diagnostic.source {
                value["source"] = json!(source);
            }
            if let Some(code) = &diagnostic.code {
                value["code"] = json!(code.to_string());
            }
            value
        })
        .collect();
    Some(json!({"uri": file_url(&path), "diagnostics": diagnostics}))
}

fn open_buffer_for_path(
    project: &Entity<Project>,
    path: &Path,
    cx: &App,
) -> Option<Entity<language::Buffer>> {
    let project = project.read(cx);
    let project_path = project.find_project_path(path, cx)?;
    project.get_open_buffer(&project_path, cx)
}

fn open_editors(workspace: &Entity<Workspace>, cx: &App) -> Value {
    let workspace = workspace.read(cx);
    let active = workspace.active_item(cx).map(|item| item.item_id());
    let tabs: Vec<Value> = workspace
        .items(cx)
        .filter_map(|item| {
            let editor = item.act_as::<Editor>(cx)?;
            let buffer = editor.read(cx).buffer().read(cx).as_singleton()?;
            let buffer = buffer.read(cx);
            let path = buffer.file()?.as_local()?.abs_path(cx);
            Some(json!({
                "uri": file_url(&path),
                "isActive": Some(item.item_id()) == active,
                "label": path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default(),
                "languageId": buffer.language().map(|language| language.name().as_ref().to_lowercase()).unwrap_or_default(),
                "isDirty": buffer.is_dirty(),
            }))
        })
        .collect();
    json!({"tabs": tabs})
}

/// Finds the range `openFile` should select: from `start_text` to the end of
/// `end_text` (or of the line), as rows and columns.
pub fn find_text_range(
    text: &str,
    start_text: Option<&str>,
    end_text: Option<&str>,
    select_to_end_of_line: bool,
) -> Option<std::ops::Range<Point>> {
    let start_text = start_text.filter(|start| !start.is_empty())?;
    let start = text.find(start_text)?;
    let mut end = match end_text.filter(|end| !end.is_empty()) {
        Some(end_text) => text[start..]
            .find(end_text)
            .map(|offset| start + offset + end_text.len())?,
        None => start + start_text.len(),
    };
    if select_to_end_of_line {
        end = text[end..]
            .find('\n')
            .map_or(text.len(), |offset| end + offset);
    }
    let point = |offset: usize| {
        let before = &text[..offset];
        let row = before.matches('\n').count() as u32;
        let column = before
            .rfind('\n')
            .map_or(offset, |newline| offset - newline - 1) as u32;
        Point::new(row, column)
    };
    Some(point(start)..point(end))
}

/// `~/.claude`, or `CLAUDE_CONFIG_DIR` when set, the CLI's own lookup.
pub fn claude_config_dir() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| paths::home_dir().join(".claude"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_notification_format() {
        let selection = SelectionInfo {
            file_path: PathBuf::from("/p/a.rs"),
            text: "fn".into(),
            start: PointUtf16::new(1, 0),
            end: PointUtf16::new(1, 2),
            rows: (1, 1),
        };
        assert_eq!(
            selection.to_notification(),
            json!({
                "text": "fn",
                "filePath": "/p/a.rs",
                "fileUrl": "file:///p/a.rs",
                "selection": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 2}, "isEmpty": false}
            })
        );
    }

    #[test]
    fn finds_open_file_ranges() {
        let text = "one\ntwo three\nfour";
        assert_eq!(
            find_text_range(text, Some("two"), None, false),
            Some(Point::new(1, 0)..Point::new(1, 3))
        );
        assert_eq!(
            find_text_range(text, Some("two"), Some("four"), false),
            Some(Point::new(1, 0)..Point::new(2, 4))
        );
        assert_eq!(
            find_text_range(text, Some("two"), None, true),
            Some(Point::new(1, 0)..Point::new(1, 9))
        );
        assert_eq!(find_text_range(text, Some("zzz"), None, false), None);
        assert_eq!(find_text_range(text, None, None, false), None);
    }

    #[test]
    fn uri_paths() {
        assert_eq!(
            path_from_uri("file:///p/a%20b.rs"),
            PathBuf::from("/p/a b.rs")
        );
        assert_eq!(path_from_uri("/p/a.rs"), PathBuf::from("/p/a.rs"));
    }
}
