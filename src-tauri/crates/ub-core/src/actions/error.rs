//! Why an action was rejected.

use ub_store::StoreError;

/// Why the core rejected a UI action. A rejected action is logged and its
/// writes roll back; `ack` still moves past it, so the mirror falls back to
/// Rust's state (SPEC UI state, Actions).
#[derive(Debug, thiserror::Error)]
pub(crate) enum ActionError {
    /// The database refused a read or write.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// The action names something that does not exist, or asks for
    /// something the core does not accept.
    #[error("{0}")]
    Invalid(&'static str),
    /// A later slice of the rewrite handles this action.
    #[error("{0} is not built yet")]
    NotYetImplemented(&'static str),
}
