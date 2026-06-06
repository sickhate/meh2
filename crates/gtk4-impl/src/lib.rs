// GPL-3.0-or-later
//! GTK4 widget builder and window management for meh2.
//!
//! Module layout:
//! - [`bindings`] — reactive attribute and loop bindings
//! - [`builder`] — widget tree construction, common props, event handlers
//! - [`widgets`] — individual GTK widget builders
//! - [`runtime`] — tokio handle, config dir, event-handler dispatch

mod bindings;
mod builder;
mod runtime;
mod widgets;

pub mod app;
pub mod css;
mod launcher;
#[cfg(feature = "rhai")]
mod rhai_widget;
pub mod window;

pub use app::{App, Cmd, connect_color_scheme, init_platform};
pub use bindings::{AnyBinding, collect_bindings};
pub use builder::build_widget;
pub use runtime::{set_config_dir, set_tokio_handle, spawn_cmd};

pub(crate) use bindings::BINDING_COLLECTOR;
pub(crate) use builder::apply_common_props;
pub(crate) use runtime::CONFIG_DIR;

