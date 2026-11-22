mod state;
mod writing;

pub(crate) use state::{
    GmailIds, GmailWrite, await_target_body, awaits_parts, awaits_target_body, begin_create,
    body_landed, composer_closed, discards_on_close, edited, gmail_changed, gmail_draft,
    gmail_gone, gmail_message, gmail_thread, gmail_write, has_skipped_gmail_change, keep,
    parts_downloaded, read_from_gmail, start_fresh, target_body_gone, written,
};
pub(crate) use writing::{DraftsWriting, WritingDraft};
