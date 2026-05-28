// GPL-3.0-or-later
//! Re-exports and helpers for gtk4-layer-shell window placement.
//!
//! The main implementation lives in `meh_gtk4::window`. This crate exists as
//! a thin facade so other crates can depend on it without pulling in all of
//! `meh_gtk4`.

pub use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
