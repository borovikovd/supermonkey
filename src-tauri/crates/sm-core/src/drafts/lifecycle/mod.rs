//! A draft's lifecycle (SPEC Drafts; UI state, Actions; Addressing; Gmail
//! errors), owned here and nowhere else. The store keeps it as plain data
//! (`sm_store::DraftLifecycle`); `state` is the only code that reads or
//! changes it, through named transitions, and `writing` keeps two writes of
//! one draft to Gmail from overlapping.
//!
//! - **Stage**, for the untouched rule: a fresh composer's first save makes
//!   a fresh draft that remembers the content it started with. Closing that
//!   composer untouched discards it; otherwise it is opened. A draft the
//!   user meant to keep (a send that came back, was refused, or was undone)
//!   and one new to the app from Gmail are kept: never untouched.
//! - **Waits**: a draft read from Gmail waits for its parts, and one filled
//!   before its target's body was stored waits for that body, before it is
//!   written to Gmail.
//! - **Gmail's copy**: the ids of the last write, and a `drafts.create`
//!   whose answer is not stored, which decide how the next write reaches
//!   Gmail.
//! - **A skipped Gmail change**: one that came while a composer had the
//!   draft open, read when it closes unless an edit saved since wins.
//!
//! Two things write a draft to Gmail: its queued upsert (one per draft; a
//! save replaces it) and, once it is sent, the outbox, which drops the
//! upserts and writes the final version itself. `writing` keeps those from
//! overlapping, and a draft reconcile leaves a draft being written alone.
//! Each write first ends the draft's waits, then goes by `gmail_write`.

mod state;
mod writing;

pub(crate) use state::{
    GmailIds, GmailWrite, await_target_body, awaits_parts, awaits_target_body, begin_create,
    body_landed, composer_closed, discards_on_close, edited, gmail_changed, gmail_draft,
    gmail_gone, gmail_message, gmail_thread, gmail_write, has_skipped_gmail_change, is_fresh,
    is_untouched, keep, parts_downloaded, read_from_gmail, start_fresh, target_body_gone, written,
};
pub(crate) use writing::{DraftsWriting, WritingDraft};
