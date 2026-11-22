use sm_domain::{
    AccountId, DraftId, DraftKind, MessageId, Millis, ReplyTarget, ThreadId, addressing,
};
use sm_mime::QuoteSource;
use sm_store::{Draft, MessageRecord, Reads, StoreError, WriteTx};

use super::lifecycle;
use super::save::message_attachments;
use super::text::{escape_text, local_offset};
use crate::actions::ActionError;

#[derive(Debug, Clone)]
pub(crate) struct NewDraft {
    pub(crate) id: DraftId,
    pub(crate) account: AccountId,
    pub(crate) kind: DraftKind,
    pub(crate) reply_all: bool,
    pub(crate) thread: Option<ThreadId>,
    pub(crate) target: Option<MessageId>,
    pub(crate) started_on_latest: bool,
}

pub(crate) fn create(tx: &mut WriteTx<'_>, new: NewDraft, now: Millis) -> Result<(), ActionError> {
    let reads = tx.read();
    if reads.drafts().get(&new.id)?.is_some() {
        return Ok(());
    }
    let Some(me) = reads.accounts().identity(new.account)? else {
        return Err(ActionError::Invalid(
            "That account is no longer in Supermonkey",
        ));
    };
    if let Some(thread) = new.thread
        && reads.threads().account_of(thread)? != Some(new.account)
    {
        return Err(ActionError::Invalid(
            "That conversation is no longer in this account",
        ));
    }
    let target = match new.target {
        Some(id) => Some(target_message(reads, id, new.account)?),
        None => None,
    };
    let filled = addressing(
        new.kind,
        target.as_ref().map(|t| ReplyTarget {
            from: &t.from,
            to: &t.to,
            cc: &t.cc,
            reply_to: &t.reply_to,
        }),
        &me,
        new.reply_all,
    );
    let mut draft = Draft::new(new.id, new.account, new.kind, now);
    draft.reply_all = new.reply_all;
    draft.thread = new.thread;
    draft.target = new.target;
    draft.started_on_latest = new.started_on_latest;
    draft.from = Some(filled.from);
    draft.to = filled.to;
    draft.cc = filled.cc;
    if let Some(t) = &target {
        answer(reads, &mut draft, t)?;
    }
    lifecycle::start_fresh(&mut draft);
    tx.drafts().put(&draft)?;
    Ok(())
}

fn target_message(
    reads: Reads<'_>,
    id: MessageId,
    account: AccountId,
) -> Result<MessageRecord, ActionError> {
    let Some(message) = reads.messages().get(id)? else {
        return Err(ActionError::Invalid("That message no longer exists"));
    };
    if message.account != account {
        return Err(ActionError::Invalid(
            "That message is no longer in this account",
        ));
    }
    Ok(message)
}

fn answer(reads: Reads<'_>, draft: &mut Draft, target: &MessageRecord) -> Result<(), StoreError> {
    draft.in_reply_to.clone_from(&target.message_id_header);
    draft.references = references(target);
    match draft.kind {
        DraftKind::Reply => draft.subject = sm_mime::reply_subject(&target.subject),
        DraftKind::Forward => draft.subject = sm_mime::forward_subject(&target.subject),
        DraftKind::New => {}
    }
    if draft.kind != DraftKind::New {
        if quote(reads, draft, target)? {
            draft.attachments = forwarded(reads, draft, target)?;
        } else {
            lifecycle::await_target_body(draft);
        }
    }
    Ok(())
}

pub(crate) fn complete(tx: &mut WriteTx<'_>, message: MessageId) -> Result<(), StoreError> {
    for id in tx.read().drafts().awaiting_body(message)? {
        complete_draft(tx, &id)?;
    }
    Ok(())
}

pub(crate) fn settle(tx: &mut WriteTx<'_>, id: &DraftId) -> Result<Option<Draft>, StoreError> {
    if !complete_draft(tx, id)?
        && let Some(record) = tx.read().drafts().get(id)?
    {
        let mut draft = record.draft;
        if lifecycle::target_body_gone(&mut draft) {
            tx.drafts().put(&draft)?;
        }
    }
    Ok(tx.read().drafts().get(id)?.map(|r| r.draft))
}

fn complete_draft(tx: &mut WriteTx<'_>, id: &DraftId) -> Result<bool, StoreError> {
    let reads = tx.read();
    let Some(record) = reads.drafts().get(id)? else {
        return Ok(false);
    };
    let mut draft = record.draft;
    if !lifecycle::awaits_target_body(&draft) {
        return Ok(false);
    }
    let Some(target) = draft
        .target
        .map(|t| reads.messages().get(t))
        .transpose()?
        .flatten()
    else {
        return Ok(false);
    };
    if !quote(reads, &mut draft, &target)? {
        return Ok(false);
    }
    let rev = tx.rev();
    let added: Vec<_> = forwarded(reads, &draft, &target)?
        .into_iter()
        .filter(|a| draft.attachments.iter().all(|have| have.id != a.id))
        .map(|mut a| {
            a.added = Some(rev);
            a
        })
        .collect();
    lifecycle::body_landed(&mut draft, &added);
    draft.attachments.extend(added);
    tx.drafts().put(&draft)?;
    Ok(true)
}

fn quote(reads: Reads<'_>, draft: &mut Draft, target: &MessageRecord) -> Result<bool, StoreError> {
    let body = reads.messages().bodies(&[target.id])?.into_iter().next();
    let whole = body.is_some();
    let html = match body {
        Some(body) => {
            let html = body
                .html
                .unwrap_or_else(|| sm_mime::text_to_html(body.text.as_deref().unwrap_or_default()));
            format!("{html}{}", body.quote_html.unwrap_or_default())
        }
        None => escape_text(&target.snippet),
    };
    let source = QuoteSource {
        from: &target.from,
        to: &target.to,
        cc: &target.cc,
        date: target.date,
        subject: &target.subject,
        html: &html,
    };
    draft.quote_html = match draft.kind {
        DraftKind::Reply => Some(sm_mime::reply_quote_html(&source, local_offset())),
        DraftKind::Forward => Some(sm_mime::forward_quote_html(&source, local_offset())),
        DraftKind::New => None,
    };
    Ok(whole)
}

fn forwarded(
    reads: Reads<'_>,
    draft: &Draft,
    target: &MessageRecord,
) -> Result<Vec<sm_store::DraftAttachment>, StoreError> {
    match draft.kind {
        DraftKind::Forward => message_attachments(reads, target.id),
        DraftKind::Reply | DraftKind::New => Ok(Vec::new()),
    }
}

fn references(target: &MessageRecord) -> String {
    let own = target.message_id_header.as_deref().unwrap_or_default();
    format!("{} {own}", target.references).trim().to_owned()
}
