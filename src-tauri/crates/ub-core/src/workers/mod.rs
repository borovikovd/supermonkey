//! The async workers (ARCHITECTURE.md, sections 4 and 8.2). Each is an
//! extension point that a later slice of the rewrite fills in; the effects
//! they consume reach them through [`crate::effects`]'s router, which drops
//! them with a debug log until then.

mod accounts;
mod drafts;
mod notifications;
mod ops;
mod outbox;
mod search;
mod snooze;
