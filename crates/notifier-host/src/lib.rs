// MIT — ported from elkowar/eww crates/notifier_host (MIT).
// Compiled only when the `systray` Cargo feature is enabled (see meh_gtk4).
//! StatusNotifierHost protocol implementation for meh's system tray.

pub mod proxy;

mod watcher;
pub use watcher::*;

mod host;
pub use host::*;

mod item;
pub use item::*;

mod icon;
pub use icon::load_icon_from_sni;

pub(crate) mod names {
    pub const WATCHER_BUS: &str = "org.kde.StatusNotifierWatcher";
    pub const WATCHER_OBJECT: &str = "/StatusNotifierWatcher";
    pub const ITEM_OBJECT: &str = "/StatusNotifierItem";
}

/// Icon resolved from a StatusNotifierItem.
/// `Send` so it can cross the `glib::MainContext::channel` into the GTK thread.
#[derive(Debug)]
pub enum IconResult {
    /// Named icon (possibly with a custom theme search path, or an absolute file path).
    Named { name: String, theme_path: Option<String> },
    /// Raw RGBA32 pixmap data (ARGB32 already converted).
    Pixmap { width: i32, height: i32, rgba: Vec<u8> },
    /// No icon available; the GTK thread should show a fallback.
    Missing,
}
