use sm_domain::{Addr, Identity, Rev, SystemLabel, ThreadId};
use sm_ipc::{
    AttachmentInfo, ConversationData, DraftSummary, MessageAddr, MessageBody, MessageSummary,
    OutboxRow,
};
use sm_store::{BodyRecord, ConversationMessage, OutboxRecord, Reads, StoreError};

pub(crate) fn outbox_row(record: OutboxRecord) -> OutboxRow {
    let o = record.outbox;
    OutboxRow {
        id: o.id,
        account: o.account,
        draft: o.draft,
        thread: o.thread,
        state: o.state,
        undo_requested: o.undo_requested,
        toast_closed: o.toast_closed,
        delayed: o.delayed,
        to: record.to,
        snippet: record.snippet,
        subject: record.subject,
        undo_entry: record.undo_entry,
    }
}

fn message_addr(addr: Addr, me: Option<&Identity>) -> MessageAddr {
    MessageAddr {
        me: me.is_some_and(|me| me.is_mine(&addr.email)),
        name: addr.name,
        email: addr.email,
    }
}

fn message_addrs(addrs: Vec<Addr>, me: Option<&Identity>) -> Vec<MessageAddr> {
    addrs.into_iter().map(|a| message_addr(a, me)).collect()
}

fn message_summary(entry: ConversationMessage, me: Option<&Identity>) -> MessageSummary {
    let ConversationMessage {
        message: m,
        attachments,
    } = entry;
    MessageSummary {
        id: m.id,
        unread: m.labels.has(SystemLabel::Unread),
        starred: m.labels.has(SystemLabel::Starred),
        from: message_addr(m.from, me),
        to: message_addrs(m.to, me),
        cc: message_addrs(m.cc, me),
        bcc: message_addrs(m.bcc, me),
        subject: m.subject,
        snippet: m.snippet,
        date: m.date,
        has_body: m.has_body,
        attachments: attachments
            .into_iter()
            .map(|a| AttachmentInfo {
                id: a.id,
                filename: a.filename,
                mime_type: a.mime_type,
                size: a.size,
                inline: a.is_inline,
            })
            .collect(),
        rev: m.rev,
    }
}

pub(crate) fn conversation(
    reads: Reads<'_>,
    thread: ThreadId,
    trash_mode: bool,
) -> Result<ConversationData, StoreError> {
    let Some(rows) = reads.threads().conversation(thread, trash_mode)? else {
        return Ok(ConversationData {
            thread,
            subject: String::new(),
            messages: Vec::new(),
            drafts: Vec::new(),
            outbox: Vec::new(),
            gone: true,
            rev: Rev(0),
        });
    };
    let me = reads.accounts().identity(rows.thread.account)?;
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
                to: d.to,
                snippet: d.snippet,
                updated_at: d.updated_at,
            }
        })
        .collect();
    Ok(ConversationData {
        thread,
        subject: rows.thread.subject,
        messages: rows
            .messages
            .into_iter()
            .map(|m| message_summary(m, me.as_ref()))
            .collect(),
        drafts,
        outbox: rows.outbox.into_iter().map(outbox_row).collect(),
        gone: false,
        rev: rows.thread.rev,
    })
}

pub(crate) fn message_body(body: BodyRecord) -> MessageBody {
    let html = body
        .html
        .unwrap_or_else(|| sm_mime::text_to_html(body.text.as_deref().unwrap_or_default()));
    MessageBody {
        id: body.id,
        html,
        quote_html: body.quote_html,
    }
}
