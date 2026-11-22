use sm_domain::{AccountId, DraftId, MessageId, Millis, Rev};
use sm_ipc::DraftFields;
use sm_store::{DraftAttachment, NewOp, OpKind, Reads, StoreError, WriteTx};

use super::lifecycle;
use super::text::snippet;
use crate::effects::Effect;
use crate::writer::WriterState;

pub(crate) struct Edit {
    pub(crate) fields: DraftFields,
    pub(crate) seen: Rev,
}

pub(crate) fn update(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    id: &DraftId,
    edit: Edit,
    now: Millis,
) -> Result<(), StoreError> {
    let Edit { fields, seen } = edit;
    let Some(record) = tx.read().drafts().get(id)? else {
        return Ok(());
    };
    if record.in_outbox {
        return Ok(());
    }
    let mut draft = record.draft;
    let DraftFields {
        from,
        to,
        cc,
        bcc,
        subject,
        body_html,
        attachments,
    } = fields;
    let old = std::mem::take(&mut draft.attachments);
    let mut attachments: Vec<DraftAttachment> = attachments
        .into_iter()
        .map(|a| {
            let stored = old.iter().find(|s| s.id == a.id);
            DraftAttachment {
                path: a.path.or_else(|| stored.and_then(|s| s.path.clone())),
                added: stored.and_then(|s| s.added),
                id: a.id,
                filename: a.filename,
                mime_type: a.mime_type,
                size: a.size,
                source: a.source,
            }
        })
        .collect();
    let unseen: Vec<DraftAttachment> = old
        .into_iter()
        .filter(|s| s.added.is_some_and(|added| added > seen))
        .filter(|s| attachments.iter().all(|a| a.id != s.id))
        .collect();
    attachments.extend(unseen);
    draft.from = from;
    draft.to = to;
    draft.cc = cc;
    draft.bcc = bcc;
    draft.subject = subject;
    draft.snippet = snippet(&body_html);
    draft.body_html = body_html;
    draft.attachments = attachments;
    draft.updated_at = now;
    lifecycle::edited(&mut draft);
    tx.drafts().put(&draft)?;
    if !lifecycle::discards_on_close(&draft) {
        queue_upsert(tx, state, draft.account, id, now)?;
    }
    Ok(())
}

pub(crate) fn queue_upsert(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    account: AccountId,
    draft: &DraftId,
    now: Millis,
) -> Result<(), StoreError> {
    tx.ops().delete_draft_upserts(draft)?;
    tx.ops().push(&NewOp {
        account,
        action: None,
        kind: OpKind::DraftUpsert,
        draft: Some(draft),
        gmail_draft: None,
        payload: &(),
        created_at: now,
    })?;
    state.effect(Effect::Ops(account));
    Ok(())
}

pub(crate) fn message_attachments(
    reads: Reads<'_>,
    message: MessageId,
) -> Result<Vec<DraftAttachment>, StoreError> {
    Ok(reads
        .messages()
        .attachments(message)?
        .into_iter()
        .filter(|a| !a.is_inline)
        .map(|a| DraftAttachment {
            id: format!("src-{}", a.id.0),
            filename: a.filename,
            mime_type: a.mime_type,
            size: a.size,
            path: a.local_path,
            source: Some(a.id),
            added: None,
        })
        .collect())
}
