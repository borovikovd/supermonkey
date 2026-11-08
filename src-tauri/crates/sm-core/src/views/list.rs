//! List rows: threads, and drafts in the Drafts list.

use sm_domain::{DraftId, LabelId, ThreadId};
use sm_ipc::ListRow;
use sm_store::{DraftRecord, Reads, StoreError, ThreadRow};

/// The row of a draft with no recipients.
const NO_RECIPIENTS: &str = "(no recipients)";
/// The label a Drafts list row carries.
const DRAFT_LABEL: &str = "DRAFT";

/// A thread's list row. Its key is its id.
pub(crate) fn thread_row(row: ThreadRow) -> ListRow {
    let starred = row.starred();
    ListRow {
        key: row.id.to_string(),
        thread: Some(row.id),
        draft: None,
        sender: row.sender,
        sender_email: row.sender_email,
        subject: row.subject,
        snippet: row.snippet,
        time: row.sort_time,
        unread: row.unread,
        starred,
        has_attachments: row.has_attachments,
        message_count: row.message_count,
        labels: row.labels.into_iter().collect(),
        snooze_until: row.snooze_until,
        split: row.split,
        rev: row.rev,
    }
}

/// A draft's row in the Drafts list. Its key is the draft id, and it shows
/// the recipients where a thread shows the sender.
pub(crate) fn draft_row(record: DraftRecord) -> ListRow {
    let draft = record.draft;
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
        draft: Some(draft.id),
        sender,
        sender_email: draft
            .to
            .into_iter()
            .next()
            .map(|a| a.email)
            .unwrap_or_default(),
        subject: draft.subject,
        snippet: draft.snippet,
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
            .into_iter()
            .map(draft_row)
            .collect())
    } else {
        let ids: Vec<ThreadId> = keys
            .iter()
            .filter_map(|k| k.as_ref().parse().ok())
            .collect();
        Ok(reads
            .threads()
            .rows(&ids)?
            .into_iter()
            .map(thread_row)
            .collect())
    }
}
