//! Command dispatch over `sm-core` (ARCHITECTURE.md, sections 3 and 13): the
//! one place the UI's commands meet the core, used by the app's Tauri
//! commands and by the debug web bridge.
//!
//! - [`dispatch`]: every command by name, with JSON arguments, parsed into
//!   `sm_ipc::Command`. Both hosts run every command through it.
//! - [`commands`]: one typed function per command, mapping core errors to
//!   the strings the UI reads.
//! - `web` (feature `web`, debug builds only): the HTTP bridge, `WebBridge`.

pub mod commands;
mod dispatch;
#[cfg(feature = "web")]
mod web;

pub use dispatch::{Answer, dispatch};
#[cfg(feature = "web")]
pub use web::{WebBridge, serve_local};
