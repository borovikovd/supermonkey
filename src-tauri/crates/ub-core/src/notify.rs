//! Which new mail notifies (SPEC Notifications), decided on the writer in
//! the poll's own transaction, so the open conversations and the stored
//! messages agree with what the poll wrote.

use std::collections::BTreeSet;

use ub_domain::{AccountId, GmailMessageId, Identity, MessageId, SystemLabel, ThreadId};
use ub_store::{MessageRecord, Reads, StoreError};

use crate::Notification;
use crate::effects::Effect;
use crate::views::setting;
use crate::writer::WriterState;

/// More new messages than this for one account in one poll become one
/// summary.
const SUMMARY_OVER: usize = 3;

/// Records a notification for each message a poll stored that history
/// reported added (`reported`), in the Inbox and unread, for any account.
/// Not for a muted account, mail from any connected account's address, or
/// a thread the focused window shows open. More than three become one
/// summary.
pub(crate) fn new_mail(
    reads: Reads<'_>,
    state: &mut WriterState,
    account: AccountId,
    stored: &[MessageId],
    reported: &BTreeSet<GmailMessageId>,
    focused: bool,
) -> Result<(), StoreError> {
    if stored.is_empty() {
        return Ok(());
    }
    if reads.settings().get(account, setting::MUTED)?.as_deref() == Some(setting::ON) {
        return Ok(());
    }
    let Some(record) = reads.accounts().get(account)? else {
        return Ok(());
    };
    let mine: Vec<Identity> = reads
        .accounts()
        .all()?
        .iter()
        .map(ub_store::AccountRecord::identity)
        .collect();
    let mut notes = Vec::new();
    for &id in stored {
        let Some(message) = reads.messages().get(id)? else {
            continue;
        };
        let notifies = reported.contains(&message.gmail_id)
            && !message.is_draft
            && message.labels.has(SystemLabel::Inbox)
            && message.labels.has(SystemLabel::Unread)
            && !mine.iter().any(|i| i.is_mine(&message.from.email))
            && !(focused && state.subs.is_open(message.thread));
        if notifies {
            notes.push(notification(
                account,
                &record.email,
                message.thread,
                &message,
            ));
        }
    }
    if notes.len() > SUMMARY_OVER {
        state.effect(Effect::Notify(Notification {
            account,
            account_email: record.email.clone(),
            thread: None,
            title: format!("{} new messages", notes.len()),
            body: record.email,
        }));
    } else {
        for note in notes {
            state.effect(Effect::Notify(note));
        }
    }
    Ok(())
}

/// One message's notification: the sender, then the subject and snippet.
fn notification(
    account: AccountId,
    email: &str,
    thread: ThreadId,
    message: &MessageRecord,
) -> Notification {
    let title = message
        .from
        .name
        .as_deref()
        .filter(|n| !n.is_empty())
        .unwrap_or(&message.from.email)
        .to_owned();
    let body = if message.subject.is_empty() {
        message.snippet.clone()
    } else {
        format!("{}\n{}", message.subject, message.snippet)
    };
    Notification {
        account,
        account_email: email.to_owned(),
        thread: Some(thread),
        title,
        body,
    }
}
