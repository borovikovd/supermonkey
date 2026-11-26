#[cfg(all(feature = "fake", not(debug_assertions)))]
compile_error!("feature `fake` is not allowed in release builds");

pub mod core;
pub mod domain;
pub mod gmail;
pub mod ipc;
pub mod mime;
pub mod session;
pub mod shell;
pub mod store;
#[cfg(feature = "fake")]
pub mod testkit;

pub use shell::run;
