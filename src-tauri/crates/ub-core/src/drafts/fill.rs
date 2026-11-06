//! A fresh composer's first save (SPEC UI state, Actions; Addressing): it
//! carries only its kind and target, and Rust fills the rest. A reply or
//! forward started before its target's body was stored quotes the snippet,
//! and gets the whole quote, and a forward its attachments, once the body
//! lands (SPEC Addressing).

use ub_domain::{
    AccountId, DraftId, DraftKind, MessageId, Millis, ReplyTarget, ThreadId, addressing,
};
use ub_mime::QuoteSource;
use ub_store::{Draft, MessageRecord, Reads, StoreError, WriteTx};

use super::lifecycle;
use super::save::message_attachments;
use super::text::{escape_text, local_offset};
use crate::actions::ActionError;

/// What a fresh composer's first save says.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NewDraft<'a> {
    pub(crate) id: &'a DraftId,
    pub(crate) account: AccountId,
    pub(crate) kind: DraftKind,
    pub(crate) reply_all: bool,
    pub(crate) thread: Option<ThreadId>,
    pub(crate) target: Option<MessageId>,
    pub(crate) started_on_latest: bool,
}

/// Creates the draft a fresh composer saves first: From, To, Cc, Subject,
/// and the quote by the rules in Addressing, and for a forward its target's
/// attachments. Those are its defaults for the untouched check. A draft that
/// exists already is left alone: the save came twice.
pub(crate) fn create(
    tx: &mut WriteTx<'_>,
    new: NewDraft<'_>,
    now: Millis,
) -> Result<(), ActionError> {
    let reads = tx.read();
    if reads.drafts().get(new.id)?.is_some() {
        return Ok(());
    }
    let Some(me) = reads.accounts().identity(new.account)? else {
        return Err(ActionError::Invalid(
            "That account is no longer in Ubermensch",
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
        Some(id) => {
            let Some(message) = reads.messages().get(id)? else {
                return Err(ActionError::Invalid("That message no longer exists"));
            };
            if message.account != new.account {
                return Err(ActionError::Invalid(
                    "That message is no longer in this account",
                ));
            }
            Some(message)
        }
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
    let mut draft = Draft::new(new.id.clone(), new.account, new.kind, now);
    draft.reply_all = new.reply_all;
    draft.thread = new.thread;
    draft.target = new.target;
    draft.started_on_latest = new.started_on_latest;
    draft.from = Some(filled.from);
    draft.to = filled.to;
    draft.cc = filled.cc;
    if let Some(t) = &target {
        draft.in_reply_to.clone_from(&t.message_id_header);
        draft.references = references(t);
        match new.kind {
            DraftKind::Reply => draft.subject = ub_mime::reply_subject(&t.subject),
            DraftKind::Forward => draft.subject = ub_mime::forward_subject(&t.subject),
            DraftKind::New => {}
        }
        if new.kind != DraftKind::New {
            if quote(reads, &mut draft, t)? {
                draft.attachments = forwarded(reads, &draft, t)?;
            } else {
                lifecycle::await_target_body(&mut draft);
            }
        }
    }
    lifecycle::start_fresh(&mut draft);
    tx.drafts().put(&draft)?;
    Ok(())
}

/// The body of `message` landed: each draft aimed at it that quotes its
/// snippet gets the whole quote, and a forward the message's attachments.
pub(crate) fn complete(tx: &mut WriteTx<'_>, message: MessageId) -> Result<(), StoreError> {
    for id in tx.read().drafts().awaiting_body(message)? {
        complete_draft(tx, &id)?;
    }
    Ok(())
}

/// Ends the draft's wait for its target's body before a write to Gmail: the
/// whole quote when the body is stored now, else the snippet's quote stays
/// (Gmail no longer has the target). Returns the draft as it is then.
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

/// Fills a draft that waits for its target's body, if that body is stored.
/// Attachments it adds count as defaults, so an untouched forward stays
/// untouched, and are marked with this `rev`, so a save from a composer
/// that has not seen them yet keeps them. Returns whether it filled it.
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

/// Sets the draft's quote of `target` by its kind: from the stored body, or
/// from the snippet when none is stored yet. Returns whether the body was
/// stored.
fn quote(reads: Reads<'_>, draft: &mut Draft, target: &MessageRecord) -> Result<bool, StoreError> {
    let body = reads.messages().bodies(&[target.id])?.into_iter().next();
    let whole = body.is_some();
    let html = match body {
        Some(body) => {
            let html = body
                .html
                .unwrap_or_else(|| ub_mime::text_to_html(body.text.as_deref().unwrap_or_default()));
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
        DraftKind::Reply => Some(ub_mime::reply_quote_html(&source, local_offset())),
        DraftKind::Forward => Some(ub_mime::forward_quote_html(&source, local_offset())),
        DraftKind::New => None,
    };
    Ok(whole)
}

/// The attachments a forward of `target` carries: the target's. None for a
/// reply.
fn forwarded(
    reads: Reads<'_>,
    draft: &Draft,
    target: &MessageRecord,
) -> Result<Vec<ub_store::DraftAttachment>, StoreError> {
    match draft.kind {
        DraftKind::Forward => message_attachments(reads, target.id),
        DraftKind::Reply | DraftKind::New => Ok(Vec::new()),
    }
}

/// The `References` a reply to `target` sends: the target's, then its own id.
fn references(target: &MessageRecord) -> String {
    let own = target.message_id_header.as_deref().unwrap_or_default();
    format!("{} {own}", target.references).trim().to_owned()
}
