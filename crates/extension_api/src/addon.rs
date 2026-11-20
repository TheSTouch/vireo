//! Vireo's addon UI API (extension API v0.9.0): commands, status bar items,
//! panels, views, editor decorations and workspace services.
//!
//! Commands are declared in `extension.toml` under `[commands.<id>]` and run
//! through [`crate::Extension::run_command`].

/// Status bar items, panels and views.
pub mod ui {
    pub use crate::wit::zed::extension::ui::*;
}
