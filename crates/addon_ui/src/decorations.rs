//! Editor decorations from addons: highlights, underlines, inlay hints, hover
//! text and gutter markers, grouped in collections owned by one addon.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use collections::{HashMap, HashSet};
use editor::{Anchor, Editor, EditorEvent, EditorMode, HighlightKey, Inlay, MultiBufferSnapshot};
use extension::{AddonDecoration, AddonPosition, AddonSeverity, AddonTextRange};
use gpui::{
    App, BorrowAppContext as _, Entity, EntityId, Global, HighlightStyle, Hsla, UnderlineStyle,
    WeakEntity, px,
};
use project::InlayId;
use text::{Bias, Point};
use theme::ActiveTheme as _;

/// Identifies a decoration collection: an addon and the collection name it chose.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CollectionKey {
    pub extension_id: Arc<str>,
    pub collection: Arc<str>,
}

/// All addon decorations, and what has been applied to which editor.
#[derive(Default)]
pub struct Decorations {
    /// Decorations by collection, then by absolute path.
    collections: HashMap<CollectionKey, HashMap<PathBuf, Vec<AddonDecoration>>>,
    /// A small number per collection, used in highlight keys.
    collection_ids: HashMap<CollectionKey, u32>,
    next_collection_id: u32,
    applied: HashMap<EntityId, Applied>,
    /// Every editor, so decorations reach the editors showing their file.
    editors: Vec<WeakEntity<Editor>>,
}

impl Global for Decorations {}

const SEVERITIES: [AddonSeverity; 4] = [
    AddonSeverity::Error,
    AddonSeverity::Warning,
    AddonSeverity::Info,
    AddonSeverity::Hint,
];

fn underline_key(collection_id: u32, severity: AddonSeverity) -> HighlightKey {
    HighlightKey::AddonUnderline(collection_id, severity as u8)
}

pub(crate) fn init(cx: &mut App) {
    cx.set_global(Decorations::default());
    cx.observe_new(|editor: &mut Editor, _, cx| {
        // A minimap shares its parent's display map, so decorating the parent
        // decorates the minimap too.
        if matches!(editor.mode(), EditorMode::Minimap { .. }) {
            return;
        }
        let editor = cx.entity();
        cx.update_global(|decorations: &mut Decorations, _| {
            decorations.editors.retain(|editor| editor.is_upgradable());
            decorations.editors.push(editor.downgrade());
        });
        let editor_id = editor.entity_id();
        cx.on_release(move |_, cx| {
            if let Some(decorations) = cx.try_global::<Decorations>()
                && decorations.applied.contains_key(&editor_id)
            {
                cx.update_global(|decorations: &mut Decorations, _| {
                    decorations.forget_editor(editor_id)
                });
            }
        })
        .detach();
        // An editor's file can change after it is created (opening, saving as).
        cx.subscribe(&editor, |_, editor, event, cx| {
            if matches!(
                event,
                EditorEvent::TitleChanged | EditorEvent::Saved | EditorEvent::FileHandleChanged
            ) {
                cx.defer(move |cx| apply_to_editor(&editor, cx));
            }
        })
        .detach();
        cx.defer(move |cx| apply_to_editor(&editor, cx));
    })
    .detach();
}

fn apply_to_editor(editor: &Entity<Editor>, cx: &mut App) {
    if cx.has_global::<Decorations>() {
        cx.update_global(|decorations: &mut Decorations, cx| {
            decorations.apply_to_editor(editor, cx)
        });
    }
}

/// Re-applies decorations to the editors showing one of `paths`, or to every
/// editor when `paths` is `None`.
pub(crate) fn refresh(paths: Option<&HashSet<PathBuf>>, cx: &mut App) {
    let Some(decorations) = cx.try_global::<Decorations>() else {
        return;
    };
    let editors: Vec<_> = decorations
        .editors
        .iter()
        .filter_map(|editor| editor.upgrade())
        .filter(|editor| {
            paths.is_none_or(|paths| {
                editor_path(editor, cx).is_some_and(|path| paths.contains(&path))
            })
        })
        .collect();
    for editor in editors {
        apply_to_editor(&editor, cx);
    }
}

/// What was applied to one editor, so it can be removed again.
#[derive(Default)]
struct Applied {
    inlays: Vec<InlayId>,
    collection_ids: Vec<u32>,
}

static NEXT_INLAY_ID: AtomicUsize = AtomicUsize::new(0);

// Gutter highlights are keyed by type, so markers are grouped per severity and
// every collection's markers are re-applied together.
struct GutterError;
struct GutterWarning;
struct GutterInfo;
struct GutterHint;

impl Decorations {
    pub fn set(&mut self, key: CollectionKey, path: PathBuf, decorations: Vec<AddonDecoration>) {
        if !self.collection_ids.contains_key(&key) {
            self.collection_ids
                .insert(key.clone(), self.next_collection_id);
            self.next_collection_id += 1;
        }
        let files = self.collections.entry(key).or_default();
        if decorations.is_empty() {
            files.remove(&path);
        } else {
            files.insert(path, decorations);
        }
    }

    /// Removes a collection and returns the paths it decorated.
    pub fn clear(&mut self, key: &CollectionKey) -> HashSet<PathBuf> {
        self.collections
            .remove(key)
            .map(|files| files.into_keys().collect())
            .unwrap_or_default()
    }

    /// Removes every collection of an addon and returns the paths they decorated.
    pub fn clear_extension(&mut self, extension_id: &str) -> HashSet<PathBuf> {
        let mut paths = HashSet::default();
        self.collections.retain(|key, files| {
            let keep = key.extension_id.as_ref() != extension_id;
            if !keep {
                paths.extend(files.keys().cloned());
            }
            keep
        });
        paths
    }

    pub fn for_path(&self, path: &Path) -> Vec<(u32, Vec<AddonDecoration>)> {
        let mut collections: Vec<_> = self
            .collections
            .iter()
            .filter_map(|(key, files)| {
                let decorations = files.get(path)?;
                Some((*self.collection_ids.get(key)?, decorations.clone()))
            })
            .collect();
        collections.sort_by_key(|(id, _)| *id);
        collections
    }

    pub fn forget_editor(&mut self, editor_id: EntityId) {
        self.applied.remove(&editor_id);
    }

    /// Re-applies every decoration to one editor, replacing what was applied before.
    pub fn apply_to_editor(&mut self, editor: &Entity<Editor>, cx: &mut App) {
        let editor_id = editor.entity_id();
        let collections = editor_path(editor, cx)
            .map(|path| self.for_path(&path))
            .unwrap_or_default();
        let old = self.applied.remove(&editor_id).unwrap_or_default();
        if collections.is_empty() && old.collection_ids.is_empty() && old.inlays.is_empty() {
            return;
        }

        let mut applied = Applied {
            inlays: Vec::new(),
            collection_ids: collections.iter().map(|(id, _)| *id).collect(),
        };
        editor.update(cx, |editor, cx| {
            for id in &old.collection_ids {
                let key = HighlightKey::Addon(*id);
                editor.clear_background_highlights(key, cx);
                editor.clear_addon_hovers(key, cx);
                for severity in SEVERITIES {
                    editor.clear_highlights(underline_key(*id, severity), cx);
                }
            }
            editor.clear_gutter_highlights::<GutterError>(cx);
            editor.clear_gutter_highlights::<GutterWarning>(cx);
            editor.clear_gutter_highlights::<GutterInfo>(cx);
            editor.clear_gutter_highlights::<GutterHint>(cx);

            let snapshot = editor.buffer().read(cx).snapshot(cx);
            let mut inlays = Vec::new();
            let mut gutter: HashMap<AddonSeverity, Vec<std::ops::Range<Anchor>>> =
                HashMap::default();
            for (collection_id, decorations) in &collections {
                let key = HighlightKey::Addon(*collection_id);
                let mut highlights = Vec::new();
                let mut highlight_color = None;
                let mut underlines: HashMap<AddonSeverity, Vec<_>> = HashMap::default();
                let mut hovers = Vec::new();
                for decoration in decorations {
                    match decoration {
                        AddonDecoration::Highlight { range, color } => {
                            highlights.push(anchor_range(&snapshot, range));
                            if highlight_color.is_none() {
                                highlight_color = color.as_deref().and_then(parse_color);
                            }
                        }
                        AddonDecoration::Underline {
                            range,
                            severity,
                            message,
                        } => {
                            let range = anchor_range(&snapshot, range);
                            underlines.entry(*severity).or_default().push(range.clone());
                            hovers.push((range, message.clone().into()));
                        }
                        AddonDecoration::InlayHint { position, text } => {
                            let id = NEXT_INLAY_ID.fetch_add(1, Ordering::Relaxed);
                            inlays.push(Inlay::addon(
                                id,
                                anchor_at(&snapshot, position),
                                text.as_str(),
                            ));
                        }
                        AddonDecoration::Hover { range, markdown } => {
                            hovers.push((anchor_range(&snapshot, range), markdown.clone().into()));
                        }
                        AddonDecoration::GutterIcon {
                            line,
                            severity,
                            tooltip,
                        } => {
                            let start = anchor_at(
                                &snapshot,
                                &AddonPosition {
                                    line: *line,
                                    column: 0,
                                },
                            );
                            gutter.entry(*severity).or_default().push(start..start);
                            if !tooltip.is_empty() {
                                let end = anchor_at(
                                    &snapshot,
                                    &AddonPosition {
                                        line: *line,
                                        column: u32::MAX,
                                    },
                                );
                                hovers.push((start..end, tooltip.clone().into()));
                            }
                        }
                    }
                }

                // A background highlight key has one color, so a collection's
                // highlights share the color of its first colored highlight.
                if !highlights.is_empty() {
                    editor.highlight_background(
                        key,
                        &highlights,
                        move |_, theme| {
                            highlight_color
                                .unwrap_or(theme.colors().editor_document_highlight_read_background)
                        },
                        cx,
                    );
                }
                for (severity, ranges) in underlines {
                    let color = severity_color(severity, cx);
                    editor.highlight_text(
                        underline_key(*collection_id, severity),
                        ranges,
                        HighlightStyle {
                            underline: Some(UnderlineStyle {
                                thickness: px(1.),
                                color: Some(color),
                                wavy: true,
                            }),
                            ..Default::default()
                        },
                        cx,
                    );
                }
                if !hovers.is_empty() {
                    editor.set_addon_hovers(key, hovers, cx);
                }
            }

            for (severity, ranges) in gutter {
                match severity {
                    AddonSeverity::Error => editor.highlight_gutter::<GutterError>(
                        ranges,
                        |cx| cx.theme().status().error,
                        cx,
                    ),
                    AddonSeverity::Warning => editor.highlight_gutter::<GutterWarning>(
                        ranges,
                        |cx| cx.theme().status().warning,
                        cx,
                    ),
                    AddonSeverity::Info => editor.highlight_gutter::<GutterInfo>(
                        ranges,
                        |cx| cx.theme().status().info,
                        cx,
                    ),
                    AddonSeverity::Hint => editor.highlight_gutter::<GutterHint>(
                        ranges,
                        |cx| cx.theme().status().hint,
                        cx,
                    ),
                }
            }

            applied.inlays = inlays.iter().map(|inlay| inlay.id).collect();
            editor.splice_inlays(&old.inlays, inlays, cx);
        });

        if !applied.inlays.is_empty() || !applied.collection_ids.is_empty() {
            self.applied.insert(editor_id, applied);
        }
    }
}

/// The absolute path of the file an editor shows, if it shows exactly one.
pub fn editor_path(editor: &Entity<Editor>, cx: &App) -> Option<PathBuf> {
    let buffer = editor.read(cx).buffer().read(cx).as_singleton()?;
    let file = buffer.read(cx).file()?;
    Some(file.as_local()?.abs_path(cx))
}

fn clip(snapshot: &MultiBufferSnapshot, position: &AddonPosition) -> Point {
    snapshot.clip_point(Point::new(position.line, position.column), Bias::Left)
}

fn anchor_at(snapshot: &MultiBufferSnapshot, position: &AddonPosition) -> Anchor {
    snapshot.anchor_before(clip(snapshot, position))
}

fn anchor_range(snapshot: &MultiBufferSnapshot, range: &AddonTextRange) -> std::ops::Range<Anchor> {
    snapshot.anchor_before(clip(snapshot, &range.start))
        ..snapshot.anchor_after(clip(snapshot, &range.end))
}

fn severity_color(severity: AddonSeverity, cx: &App) -> Hsla {
    let status = cx.theme().status();
    match severity {
        AddonSeverity::Error => status.error,
        AddonSeverity::Warning => status.warning,
        AddonSeverity::Info => status.info,
        AddonSeverity::Hint => status.hint,
    }
}

/// Parses `#rrggbb` or `#rrggbbaa`.
pub fn parse_color(color: &str) -> Option<Hsla> {
    let hex = color.strip_prefix('#')?;
    let value = u32::from_str_radix(hex, 16).ok()?;
    let rgba = match hex.len() {
        6 => (value << 8) | 0xff,
        8 => value,
        _ => return None,
    };
    Some(gpui::rgba(rgba).into())
}
