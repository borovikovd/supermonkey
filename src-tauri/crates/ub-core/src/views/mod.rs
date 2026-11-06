//! Store records to IPC read models, in one place: first loads, patches,
//! and queries all build what the UI reads here, and later slices reuse it.
//!
//! Each function reads through [`ub_store::Reads`], so it works the same on
//! the writer's snapshot, a read-pool transaction, and inside a write.

mod account;
mod composer;
mod conversation;
mod list;

pub(crate) use account::{
    APP_READS, BADGE_READS, SUMMARY_READS, account_summary, app_state, badge_count, empty_summary,
    setting,
};
pub(crate) use composer::draft_state;
pub(crate) use conversation::{conversation, message_body, outbox_row};
pub(crate) use list::rows_for_keys;
