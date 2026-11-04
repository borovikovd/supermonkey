//! A fresh composer's first save (SPEC UI state, Actions; Addressing): it
//! carries only its kind and target, and Rust fills the rest.

use ub_domain::{
    AccountId, DraftContent, DraftId, DraftKind, MessageId, Millis, ReplyTarget, ThreadId,
    addressing,
};
use ub_mime::QuoteSource;
use ub_store::{Draft, MessageRecord, Reads, StoreError, WriteTx};

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
        return Err(ActionError::Invalid("no such account"));
    };
    if let Some(thread) = new.thread
        && reads.threads().account_of(thread)? != Some(new.account)
    {
        return Err(ActionError::Invalid("the thread is not in the account"));
    }
    let target = match new.target {
        Some(id) => {
            let Some(message) = reads.messages().get(id)? else {
                return Err(ActionError::Invalid("no such message"));
            };
            if message.account != new.account {
                return Err(ActionError::Invalid("the message is not in the account"));
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
    draft.started_fresh = true;
    draft.from = Some(filled.from);
    draft.to = filled.to;
    draft.cc = filled.cc;
    if let Some(t) = &target {
        draft.in_reply_to.clone_from(&t.message_id_header);
        draft.references = references(t);
        let quoted = quoted_html(reads, t)?;
        let source = QuoteSource {
            from: &t.from,
            to: &t.to,
            cc: &t.cc,
            date: t.date,
            subject: &t.subject,
            html: &quoted,
        };
        match new.kind {
            DraftKind::Reply => {
                draft.subject = ub_mime::reply_subject(&t.subject);
                draft.quote_html = Some(ub_mime::reply_quote_html(&source, local_offset()));
            }
            DraftKind::Forward => {
                draft.subject = ub_mime::forward_subject(&t.subject);
                draft.quote_html = Some(ub_mime::forward_quote_html(&source, local_offset()));
                draft.attachments = message_attachments(reads, t.id)?;
            }
            DraftKind::New => {}
        }
    }
    draft.defaults = Some(DraftContent {
        body_html: String::new(),
        ..draft.content()
    });
    tx.drafts().put(&draft)?;
    Ok(())
}

/// The `References` a reply to `target` sends: the target's, then its own id.
fn references(target: &MessageRecord) -> String {
    let own = target.message_id_header.as_deref().unwrap_or_default();
    format!("{} {own}", target.references).trim().to_owned()
}

/// The HTML a quote of `target` shows: its stored body with its own quoted
/// history, or its snippet when the body is not stored yet.
fn quoted_html(reads: Reads<'_>, target: &MessageRecord) -> Result<String, StoreError> {
    let body = reads.messages().bodies(&[target.id])?.into_iter().next();
    Ok(match body {
        Some(body) => {
            let html = body
                .html
                .unwrap_or_else(|| ub_mime::text_to_html(body.text.as_deref().unwrap_or_default()));
            format!("{html}{}", body.quote_html.unwrap_or_default())
        }
        None => escape_text(&target.snippet),
    })
}
