//! Subscriptions (SPEC UI state, Subscriptions; ARCHITECTURE.md, section
//! 5.2): the registry of what the UI shows, which subscriptions a commit
//! affects, and the first loads and patches built from the committed
//! snapshot.
//!
//! A list subscription remembers the ids it last sent. After each commit
//! that may change it, its ids are read again from the snapshot and diffed
//! into removals and moves, so the mirror rebuilds the same list.

mod affects;
mod build;
mod diff;
mod lists;
mod registry;

#[cfg(test)]
mod tests;

pub(crate) use affects::affects_badge;
pub(crate) use registry::Registry;
