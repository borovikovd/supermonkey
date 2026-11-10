//! Drafts on the writer side (SPEC Compose: Reply composer, Addressing,
//! Drafts; UI state, Actions): a fresh composer's first save filled by the
//! addressing rules, later saves and the upsert they queue, the untouched
//! rule when a composer closes, discard, and what Gmail's copy of a draft
//! changes here.
//!
//! A draft's lifecycle (its stage, what it waits for, its Gmail copy, and
//! who may write it to Gmail) is `lifecycle`'s alone: everything else goes
//! through its transitions.
//!
//! The Gmail side (writing, reading back, downloading parts) is in
//! `workers::drafts`; everything here but `lifecycle::writing` runs on the
//! writer thread.

mod close;
mod fill;
mod gmail;
pub(crate) mod lifecycle;
mod save;
mod text;

pub(crate) use close::{close, discard, on_launch};
pub(crate) use fill::{NewDraft, complete, create, settle};
pub(crate) use gmail::{
    GmailDraft, Reconcile, apply_gmail_draft, defer_if_open, delete_gmail_draft, drop_gone,
    store_ids,
};
pub(crate) use lifecycle::{DraftsWriting, WritingDraft};
pub(crate) use save::{Edit, queue_upsert, update};
