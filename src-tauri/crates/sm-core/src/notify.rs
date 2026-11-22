use std::collections::BTreeSet;

use sm_domain::{
    AccountId, GmailMessageId, Identity, MessageId, SettingKey, SystemLabel, ThreadId,
};
use sm_store::{MessageRecord, Reads, StoreError};

use crate::Notification;
use crate::effects::Effect;
use crate::writer::WriterState;

const SUMMARY_OVER: usize = 3;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Arrivals<'a> {
    pub(crate) stored: &'a [MessageId],
    pub(crate) reported: &'a BTreeSet<GmailMessageId>,
}

pub(crate) fn new_mail(
    reads: Reads<'_>,
    state: &mut WriterState,
    account: AccountId,
    arrivals: Arrivals<'_>,
    focused: bool,
) -> Result<(), StoreError> {
    let Arrivals { stored, reported } = arrivals;
    if stored.is_empty() {
        return Ok(());
    }
    if reads.settings().is_on(account, SettingKey::Muted)? {
        return Ok(());
    }
    let Some(record) = reads.accounts().get(account)? else {
        return Ok(());
    };
    let mine: Vec<Identity> = reads
        .accounts()
        .all()?
        .into_iter()
        .map(sm_store::AccountRecord::into_identity)
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
                message,
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

fn notification(
    account: AccountId,
    email: &str,
    thread: ThreadId,
    message: MessageRecord,
) -> Notification {
    let title = message
        .from
        .name
        .filter(|n| !n.is_empty())
        .unwrap_or(message.from.email);
    let body = if message.subject.is_empty() {
        message.snippet
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
