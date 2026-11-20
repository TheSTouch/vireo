use std::str::FromStr as _;

use extension::{AddonDock, AddonPanelManifestEntry};
use gpui::{
    Action, App, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, Pixels,
    Render, Window, px,
};
use schemars::JsonSchema;
use serde::Deserialize;
use ui::{IconName, prelude::*};
use workspace::{
    Workspace,
    dock::{DockPosition, Panel, PanelEvent},
};

use crate::{AddonRegistry, AddonViewKey, view::AddonView};

/// How many addon panels a workspace can show at once.
///
/// The dock identifies panels by their Rust type, so each addon panel takes one
/// of a fixed set of `AddonPanel<SLOT>` types.
pub const PANEL_SLOTS: usize = 8;

const PANEL_KEYS: [&str; PANEL_SLOTS] = [
    "AddonPanel0",
    "AddonPanel1",
    "AddonPanel2",
    "AddonPanel3",
    "AddonPanel4",
    "AddonPanel5",
    "AddonPanel6",
    "AddonPanel7",
];

/// Runs `$body` with `$panel` naming the `AddonPanel` type of slot `$slot`.
macro_rules! with_panel_slot {
    ($slot:expr, $panel:ident => $body:expr) => {
        match $slot {
            0 => {
                type $panel = $crate::panel::AddonPanel<0>;
                $body
            }
            1 => {
                type $panel = $crate::panel::AddonPanel<1>;
                $body
            }
            2 => {
                type $panel = $crate::panel::AddonPanel<2>;
                $body
            }
            3 => {
                type $panel = $crate::panel::AddonPanel<3>;
                $body
            }
            4 => {
                type $panel = $crate::panel::AddonPanel<4>;
                $body
            }
            5 => {
                type $panel = $crate::panel::AddonPanel<5>;
                $body
            }
            6 => {
                type $panel = $crate::panel::AddonPanel<6>;
                $body
            }
            7 => {
                type $panel = $crate::panel::AddonPanel<7>;
                $body
            }
            _ => {}
        }
    };
}

/// Shows or hides an addon panel.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = addon)]
#[serde(deny_unknown_fields)]
pub struct TogglePanel {
    /// The ID of the extension that declares the panel.
    pub extension: String,
    /// The panel ID, as declared under `[panels.<id>]`.
    pub panel: String,
}

/// A dock panel showing one addon view.
pub struct AddonPanel<const SLOT: usize> {
    key: AddonViewKey,
    title: SharedString,
    icon: IconName,
    position: DockPosition,
    focus_handle: FocusHandle,
    view: Entity<AddonView>,
}

impl<const SLOT: usize> AddonPanel<SLOT> {
    pub fn new(
        key: AddonViewKey,
        panel: AddonPanelManifestEntry,
        registry: Entity<AddonRegistry>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let view = cx.new(|cx| AddonView::new(key.clone(), panel.view, registry, window, cx));
        Self {
            title: panel.title.clone().into(),
            icon: panel
                .icon
                .as_deref()
                .and_then(|icon| IconName::from_str(icon).ok())
                .unwrap_or(IconName::Blocks),
            position: match panel.dock {
                AddonDock::Left => DockPosition::Left,
                AddonDock::Right => DockPosition::Right,
                AddonDock::Bottom => DockPosition::Bottom,
            },
            key,
            focus_handle: cx.focus_handle(),
            view,
        }
    }

    pub fn key(&self) -> &AddonViewKey {
        &self.key
    }

    pub fn view(&self) -> &Entity<AddonView> {
        &self.view
    }
}

impl<const SLOT: usize> EventEmitter<PanelEvent> for AddonPanel<SLOT> {}

impl<const SLOT: usize> Focusable for AddonPanel<SLOT> {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl<const SLOT: usize> Render for AddonPanel<SLOT> {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .key_context("AddonPanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .child(Label::new(self.title.clone()).size(LabelSize::Small)),
            )
            .child(self.view.clone())
    }
}

impl<const SLOT: usize> Panel for AddonPanel<SLOT> {
    fn persistent_name() -> &'static str {
        PANEL_KEYS[SLOT]
    }

    fn panel_key() -> &'static str {
        PANEL_KEYS[SLOT]
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        self.position
    }

    fn position_is_valid(&self, _position: DockPosition) -> bool {
        true
    }

    fn set_position(
        &mut self,
        position: DockPosition,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.position = position;
        cx.notify();
    }

    fn default_size(&self, _window: &Window, _cx: &App) -> Pixels {
        match self.position {
            DockPosition::Bottom => px(240.),
            DockPosition::Left | DockPosition::Right => px(280.),
        }
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(self.icon)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some(interned_title(&self.title))
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(TogglePanel {
            extension: self.key.extension_id.to_string(),
            panel: self.key.view_id.to_string(),
        })
    }

    fn activation_priority(&self) -> u32 {
        // Zed's own panels use priorities below 1000; each panel needs a unique one.
        1000 + SLOT as u32
    }
}

/// `Panel::icon_tooltip` needs a `&'static str`. Titles are interned so each
/// distinct title is leaked once, however often panels are rebuilt.
fn interned_title(title: &str) -> &'static str {
    static TITLES: std::sync::LazyLock<parking_lot::Mutex<collections::HashSet<&'static str>>> =
        std::sync::LazyLock::new(Default::default);
    let mut titles = TITLES.lock();
    if let Some(title) = titles.get(title) {
        return title;
    }
    let title: &'static str = Box::leak(title.to_owned().into_boxed_str());
    titles.insert(title);
    title
}

/// Makes the workspace's addon panels match the registry: adds panels for new
/// addon panels, and removes panels whose addon has unloaded.
pub(crate) fn sync_panels(
    workspace: &mut Workspace,
    registry: &Entity<AddonRegistry>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    for slot in 0..PANEL_SLOTS {
        let wanted = registry.read(cx).panel_in_slot(slot);
        with_panel_slot!(slot, SlotPanel => {
            let current = workspace.panel::<SlotPanel>(cx);
            let current_key = current.as_ref().map(|panel| panel.read(cx).key().clone());
            if current_key != wanted {
                if let Some(current) = current {
                    workspace.remove_panel(&current, window, cx);
                }
                if let Some(key) = wanted
                    && let Some(entry) = registry.read(cx).panel(&key)
                {
                    let registry = registry.clone();
                    let panel = cx.new(|cx| SlotPanel::new(key, entry, registry, window, cx));
                    workspace.add_panel(panel, window, cx);
                }
            }
        });
    }
}

pub(crate) fn toggle_panel(
    workspace: &mut Workspace,
    action: &TogglePanel,
    registry: &Entity<AddonRegistry>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let key = AddonViewKey::new(action.extension.as_str(), action.panel.as_str());
    let Some(slot) = registry.read(cx).slot_of_panel(&key) else {
        return;
    };
    with_panel_slot!(slot, SlotPanel => {
        workspace.toggle_panel_focus::<SlotPanel>(window, cx);
    });
}
