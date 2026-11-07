//! Conversations, their outbox rows, and message bodies.

use ub_domain::{AccountId, Rev, SystemLabel, ThreadId};
use ub_ipc::{
    Addr, AttachmentInfo, ConversationData, DraftSummary, MessageBody, MessageSummary, OutboxRow,
};
use ub_store::{BodyRecord, ConversationMessage, OutboxRecord, Reads, StoreError};

fn addrs(list: Vec<ub_domain::Addr>) -> Vec<Addr> {
    list.into_iter().map(Addr::from).collect()
}

/// A send as its toast and the pending message show it.
pub(crate) fn outbox_row(record: OutboxRecord) -> OutboxRow {
    let o = record.outbox;
    OutboxRow {
        id: o.id,
        account: o.account,
        draft: o.draft,
        thread: o.thread,
        send_after: o.send_after,
        state: o.state,
        sent_message_gmail_id: o.sent_message_id,
        undo_requested: o.undo_requested,
        toast_closed: o.toast_closed,
        delayed: o.delayed,
        to: addrs(record.to),
        snippet: record.snippet,
        subject: record.subject,
        undo_entry: record.undo_entry,
    }
}

fn message_summary(entry: ConversationMessage) -> MessageSummary {
    let ConversationMessage {
        message: m,
        attachments,
    } = entry;
    MessageSummary {
        id: m.id,
        unread: m.labels.has(SystemLabel::Unread),
        starred: m.labels.has(SystemLabel::Starred),
        in_trash: m.labels.has(SystemLabel::Trash),
        in_spam: m.labels.has(SystemLabel::Spam),
        from: Addr::from(m.from),
        to: addrs(m.to),
        cc: addrs(m.cc),
        bcc: addrs(m.bcc),
        subject: m.subject,
        snippet: m.snippet,
        date: m.date,
        has_body: m.has_body,
        plain: m.is_plain,
        attachments: attachments
            .into_iter()
            .map(|a| AttachmentInfo {
                id: a.id,
                filename: a.filename,
                mime_type: a.mime_type,
                size: a.size,
                inline: a.is_inline,
                downloaded: a.local_path.is_some(),
            })
            .collect(),
        rev: m.rev,
    }
}

/// A conversation as its subscription shows it. `trash_mode` lists the
/// messages in Trash and Spam too. A thread that is gone gives `gone`.
pub(crate) fn conversation(
    reads: Reads<'_>,
    thread: ThreadId,
    trash_mode: bool,
) -> Result<ConversationData, StoreError> {
    let Some(rows) = reads.threads().conversation(thread, trash_mode)? else {
        return Ok(ConversationData {
            thread,
            account: AccountId::NONE,
            subject: String::new(),
            labels: Vec::new(),
            snooze_until: None,
            messages: Vec::new(),
            drafts: Vec::new(),
            outbox: Vec::new(),
            gone: true,
            rev: Rev(0),
        });
    };
    let drafts = rows
        .drafts
        .into_iter()
        .map(|record| {
            let d = record.draft;
            DraftSummary {
                id: d.id,
                kind: d.kind,
                target: d.target,
                started_on_latest: d.started_on_latest,
                to: addrs(d.to),
                snippet: d.snippet,
                updated_at: d.updated_at,
            }
        })
        .collect();
    Ok(ConversationData {
        thread,
        account: rows.thread.account,
        subject: rows.thread.subject,
        labels: rows.thread.labels.into_iter().collect(),
        snooze_until: rows.thread.snooze_until,
        messages: rows.messages.into_iter().map(message_summary).collect(),
        drafts,
        outbox: rows.outbox.into_iter().map(outbox_row).collect(),
        gone: false,
        rev: rows.thread.rev,
    })
}

/// A stored body for the message view. Plain-text mail becomes HTML here.
pub(crate) fn message_body(body: BodyRecord) -> MessageBody {
    let html = body
        .html
        .unwrap_or_else(|| ub_mime::text_to_html(body.text.as_deref().unwrap_or_default()));
    MessageBody {
        id: body.id,
        html,
        quote_html: body.quote_html,
    }
}
