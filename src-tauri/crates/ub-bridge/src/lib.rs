//! Command dispatch over `ub-core` (ARCHITECTURE.md, sections 3 and 13): the
//! one place the UI's commands meet the core, used by the Tauri commands and
//! by the debug web bridge.
//!
//! - [`commands`]: one typed function per command, mapping core errors to
//!   the strings the UI reads. The Tauri commands call these.
//! - [`dispatch`]: the same commands by name, with JSON arguments, as the
//!   web bridge receives them.
//! - `web` (feature `web`, debug builds only): the HTTP bridge, `WebBridge`.

pub mod commands;
mod dispatch;
#[cfg(feature = "web")]
mod web;

pub(crate) use dispatch::is_sign_in;
pub use dispatch::{Reply, dispatch};
#[cfg(feature = "web")]
pub use web::{WebBridge, serve_local};
