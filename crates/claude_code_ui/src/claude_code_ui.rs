pub mod conversation;
pub mod process;
pub mod protocol;
pub mod session;
pub mod settings;

use ::settings::Settings as _;
use gpui::App;

pub use crate::settings::ClaudeCodeSettings;

pub fn init(cx: &mut App) {
    ClaudeCodeSettings::register(cx);
    claude_ide::init(cx);
}
