pub mod commands;
mod dispatch;
#[cfg(feature = "web")]
mod web;

pub use dispatch::{Answer, dispatch};
#[cfg(feature = "web")]
pub use web::{WebBridge, serve_local};
