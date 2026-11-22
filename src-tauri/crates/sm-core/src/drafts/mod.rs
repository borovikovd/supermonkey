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
