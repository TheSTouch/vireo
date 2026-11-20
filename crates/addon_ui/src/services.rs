//! Workspace services for addons: the active editor, opening files,
//! notifications, prompts, workspace folders, file watches, settings and
//! storage.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result};
use collections::HashMap;
use db::kvp::KeyValueStore;
use editor::{Editor, SelectionEffects, ToPoint as _};
use extension::{
    AddonActiveEditor, AddonNotificationLevel, AddonPosition, AddonQuickPickItem, AddonTextRange,
};
use futures::{FutureExt as _, StreamExt as _, future::BoxFuture};
use gpui::{App, AppContext as _, Entity, SharedString, WindowHandle};
use settings::{RegisterSetting, Settings};
use text::{Bias, Point};
use workspace::{
    MultiWorkspace, Workspace,
    notifications::{NotificationId, simple_message_notification::MessageNotification},
};

use crate::{AddonRegistry, prompt};

/// The `addons` settings: each addon's settings by extension ID.
#[derive(Clone, Debug, Default, RegisterSetting)]
pub struct AddonSettings(pub HashMap<String, serde_json::Value>);

impl Settings for AddonSettings {
    fn from_settings(content: &settings::SettingsContent) -> Self {
        Self(
            content
                .addons
                .clone()
                .unwrap_or_default()
                .into_iter()
                .collect(),
        )
    }
}

/// The window of the active workspace, or of the first workspace when no
/// window is active.
fn workspace_window(cx: &App) -> Option<WindowHandle<MultiWorkspace>> {
    cx.active_window()
        .and_then(|window| window.downcast::<MultiWorkspace>())
        .or_else(|| {
            cx.windows()
                .into_iter()
                .find_map(|window| window.downcast::<MultiWorkspace>())
        })
}

fn active_workspace(cx: &App) -> Option<Entity<Workspace>> {
    let window = workspace_window(cx)?;
    window
        .read(cx)
        .ok()
        .map(|multi_workspace| multi_workspace.workspace().clone())
}

fn active_editor_entity(cx: &App) -> Option<Entity<Editor>> {
    active_workspace(cx)?
        .read(cx)
        .active_item(cx)?
        .act_as::<Editor>(cx)
}

pub(crate) fn active_editor(cx: &App) -> Option<AddonActiveEditor> {
    let editor = active_editor_entity(cx)?;
    let path = crate::decorations::editor_path(&editor, cx);
    let editor = editor.read(cx);
    let snapshot = editor.buffer().read(cx).snapshot(cx);
    let selection = editor.selections.newest_anchor();
    let start = selection.start.to_point(&snapshot);
    let end = selection.end.to_point(&snapshot);
    let position = |point: Point| AddonPosition {
        line: point.row,
        column: point.column,
    };
    Some(AddonActiveEditor {
        path,
        selection: AddonTextRange {
            start: position(start),
            end: position(end),
        },
        selected_text: snapshot.text_for_range(start..end).collect(),
    })
}

pub(crate) fn active_editor_text(cx: &App) -> Option<String> {
    Some(active_editor_entity(cx)?.read(cx).text(cx))
}

pub(crate) fn open_file(
    path: PathBuf,
    position: Option<AddonPosition>,
    cx: &mut App,
) -> BoxFuture<'static, Result<()>> {
    let (tx, rx) = futures::channel::oneshot::channel();
    let Some(window) = workspace_window(cx) else {
        return async { Err(anyhow::anyhow!("no workspace is open")) }.boxed();
    };
    let opened = window.update(cx, |multi_workspace, window, cx| {
        multi_workspace.workspace().update(cx, |workspace, cx| {
            workspace.open_abs_path(path, Default::default(), window, cx)
        })
    });
    match opened {
        Ok(opened) => {
            cx.spawn(async move |cx| {
                let result = async {
                    let item = opened.await?;
                    if let Some(position) = position
                        && let Some(editor) = cx.update(|cx| item.act_as::<Editor>(cx))
                    {
                        window.update(cx, |_, window, cx| {
                            editor.update(cx, |editor, cx| {
                                let snapshot = editor.buffer().read(cx).snapshot(cx);
                                let point = snapshot.clip_point(
                                    Point::new(position.line, position.column),
                                    Bias::Left,
                                );
                                editor.change_selections(
                                    SelectionEffects::default(),
                                    window,
                                    cx,
                                    |selections| selections.select_ranges([point..point]),
                                );
                            })
                        })?;
                    }
                    anyhow::Ok(())
                }
                .await;
                tx.send(result).ok();
            })
            .detach();
        }
        Err(error) => {
            tx.send(Err(error)).ok();
        }
    }
    async move { rx.await.context("the workspace closed")? }.boxed()
}

struct AddonNotification;

pub(crate) fn show_notification(
    extension_id: Arc<str>,
    level: AddonNotificationLevel,
    message: String,
    cx: &mut App,
) {
    let Some(workspace) = active_workspace(cx) else {
        log::info!("addon {extension_id}: {message}");
        return;
    };
    let message = match level {
        AddonNotificationLevel::Info => message,
        AddonNotificationLevel::Warning => format!("Warning: {message}"),
        AddonNotificationLevel::Error => format!("Error: {message}"),
    };
    workspace.update(cx, |workspace, cx| {
        workspace.show_notification(
            NotificationId::composite::<AddonNotification>(SharedString::from(
                extension_id.to_string(),
            )),
            cx,
            |cx| cx.new(|cx| MessageNotification::new(message, cx)),
        );
    });
}

fn prompt_in_workspace(
    cx: &mut App,
    open: impl FnOnce(
        &mut Workspace,
        &mut gpui::Window,
        &mut gpui::Context<Workspace>,
    ) -> futures::channel::oneshot::Receiver<Option<String>>,
) -> BoxFuture<'static, Option<String>> {
    let Some(window) = workspace_window(cx) else {
        return async { None }.boxed();
    };
    let answer = window.update(cx, |multi_workspace, window, cx| {
        multi_workspace
            .workspace()
            .update(cx, |workspace, cx| open(workspace, window, cx))
    });
    match answer {
        Ok(answer) => async move { answer.await.ok().flatten() }.boxed(),
        Err(_) => async { None }.boxed(),
    }
}

pub(crate) fn quick_pick(
    items: Vec<AddonQuickPickItem>,
    placeholder: Option<String>,
    cx: &mut App,
) -> BoxFuture<'static, Option<String>> {
    prompt_in_workspace(cx, move |workspace, window, cx| {
        prompt::quick_pick(workspace, items, placeholder, window, cx)
    })
}

pub(crate) fn input_box(
    prompt_text: String,
    value: Option<String>,
    cx: &mut App,
) -> BoxFuture<'static, Option<String>> {
    prompt_in_workspace(cx, move |workspace, window, cx| {
        prompt::input_box(workspace, prompt_text, value, window, cx)
    })
}

pub(crate) fn workspace_folders(cx: &App) -> Vec<PathBuf> {
    let Some(workspace) = active_workspace(cx) else {
        return Vec::new();
    };
    workspace
        .read(cx)
        .visible_worktrees(cx)
        .map(|worktree| worktree.read(cx).abs_path().to_path_buf())
        .collect()
}

/// Starts watching `path` for an addon. Returns the watch ID.
pub(crate) fn watch_path(extension_id: Arc<str>, path: PathBuf, cx: &mut App) -> u64 {
    let Some(registry) = AddonRegistry::global(cx) else {
        return 0;
    };
    let fs = active_workspace(cx)
        .map(|workspace| workspace.read(cx).app_state().fs.clone())
        .or_else(|| workspace::AppState::try_global(cx).map(|app_state| app_state.fs.clone()));
    registry.update(cx, |registry, cx| {
        let watch_id = registry.next_watch_id();
        let Some(fs) = fs else {
            return watch_id;
        };
        let addon = registry.addon(&extension_id);
        let task = cx.background_spawn(async move {
            let (mut events, _watcher) = fs.watch(&path, Duration::from_millis(100)).await;
            while let Some(events) = events.next().await {
                let Some(addon) = addon.as_ref() else {
                    continue;
                };
                let paths = events
                    .into_iter()
                    .map(|event| event.path.to_string_lossy().into_owned())
                    .collect();
                if let Err(error) = addon.addon_files_changed(watch_id, paths).await {
                    log::error!("addon failed to handle file changes: {error:#}");
                }
            }
        });
        registry.add_watch(extension_id, watch_id, task);
        watch_id
    })
}

pub(crate) fn unwatch_path(extension_id: Arc<str>, watch_id: u64, cx: &mut App) {
    if let Some(registry) = AddonRegistry::global(cx) {
        registry.update(cx, |registry, _| {
            registry.remove_watch(&extension_id, watch_id)
        });
    }
}

pub(crate) fn addon_settings(extension_id: &str, cx: &App) -> Option<String> {
    let value = AddonSettings::get_global(cx).0.get(extension_id)?;
    serde_json::to_string(value).ok()
}

fn storage_namespace(extension_id: &str) -> String {
    format!("addon:{extension_id}")
}

pub(crate) fn storage_get(extension_id: &str, key: &str, cx: &App) -> Option<String> {
    let store = KeyValueStore::global(cx);
    let namespace = storage_namespace(extension_id);
    store
        .scoped(&namespace)
        .read(key)
        .inspect_err(|error| log::error!("addon storage read failed: {error:#}"))
        .ok()
        .flatten()
}

pub(crate) fn storage_set(
    extension_id: &str,
    key: String,
    value: Option<String>,
    cx: &App,
) -> BoxFuture<'static, Result<()>> {
    let store = KeyValueStore::global(cx);
    let namespace = storage_namespace(extension_id);
    async move {
        let scoped = store.scoped(&namespace);
        match value {
            Some(value) => scoped.write(key, value).await,
            None => scoped.delete(key).await,
        }
    }
    .boxed()
}
