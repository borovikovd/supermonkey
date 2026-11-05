//! List rows: threads, and drafts in the Drafts list.

use ub_domain::{DraftId, LabelId, ThreadId};
use ub_ipc::ListRow;
use ub_store::{DraftRecord, Reads, StoreError, ThreadRow};

/// The row of a draft with no recipients.
const NO_RECIPIENTS: &str = "(no recipients)";
/// The label a Drafts list row carries.
const DRAFT_LABEL: &str = "DRAFT";

/// A thread's list row. Its key is its id.
pub(crate) fn thread_row(row: &ThreadRow) -> ListRow {
    ListRow {
        key: row.id.to_string(),
        thread: Some(row.id),
        draft: None,
        sender: row.sender.clone(),
        sender_email: row.sender_email.clone(),
        subject: row.subject.clone(),
        snippet: row.snippet.clone(),
        time: row.sort_time,
        unread: row.unread,
        starred: row.starred(),
        has_attachments: row.has_attachments,
        message_count: row.message_count,
        labels: row.labels.iter().cloned().collect(),
        snooze_until: row.snooze_until,
        split: row.split,
        rev: row.rev,
    }
}

/// A draft's row in the Drafts list. Its key is the draft id, and it shows
/// the recipients where a thread shows the sender.
pub(crate) fn draft_row(record: &DraftRecord) -> ListRow {
    let draft = &record.draft;
    let sender = if draft.to.is_empty() {
        NO_RECIPIENTS.to_owned()
    } else {
        draft
            .to
            .iter()
            .map(|a| {
                a.name
                    .as_deref()
                    .filter(|n| !n.is_empty())
                    .unwrap_or(&a.email)
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    ListRow {
        key: draft.id.to_string(),
        thread: draft.thread,
        draft: Some(draft.id.clone()),
        sender,
        sender_email: draft
            .to
            .first()
            .map(|a| a.email.clone())
            .unwrap_or_default(),
        subject: draft.subject.clone(),
        snippet: draft.snippet.clone(),
        time: draft.updated_at,
        unread: false,
        starred: false,
        has_attachments: !draft.attachments.is_empty(),
        message_count: 1,
        labels: vec![LabelId::from(DRAFT_LABEL)],
        snooze_until: None,
        split: None,
        rev: record.rev,
    }
}

/// The rows for list keys: draft ids in the Drafts list, thread ids in
/// every other. Unknown keys are left out.
pub(crate) fn rows_for_keys(
    reads: Reads<'_>,
    drafts: bool,
    keys: &[impl AsRef<str>],
) -> Result<Vec<ListRow>, StoreError> {
    if drafts {
        let ids: Vec<DraftId> = keys.iter().map(|k| DraftId::from(k.as_ref())).collect();
        Ok(reads
            .drafts()
            .get_many(&ids)?
            .iter()
            .map(draft_row)
            .collect())
    } else {
        let ids: Vec<ThreadId> = keys
            .iter()
            .filter_map(|k| k.as_ref().parse().ok())
            .collect();
        Ok(reads.threads().rows(&ids)?.iter().map(thread_row).collect())
    }
}
