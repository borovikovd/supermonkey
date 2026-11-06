//! Drafts on the writer side (SPEC Compose: Reply composer, Addressing,
//! Drafts; UI state, Actions): a fresh composer's first save filled by the
//! addressing rules, later saves and the upsert they queue, the untouched
//! rule when a composer closes, discard, and what Gmail's copy of a draft
//! changes here.
//!
//! The Gmail side (writing, reading back, downloading parts) is in
//! `workers::drafts`; everything here runs on the writer thread.

mod close;
mod fill;
mod gmail;
mod save;
mod text;
mod writing;

pub(crate) use close::{close, discard, on_launch};
pub(crate) use fill::{NewDraft, complete, create, settle};
pub(crate) use gmail::{
    GmailDraft, Reconcile, apply_gmail_draft, defer_if_open, drop_gone, store_ids,
};
pub(crate) use save::{queue_upsert, update};
pub(crate) use writing::{DraftsWriting, WritingDraft};
