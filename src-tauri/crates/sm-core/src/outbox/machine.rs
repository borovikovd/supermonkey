use sm_domain::{DraftId, GmailDraftId, GmailMessageId, NoticeKind, OutboxState, ThreadId};
use sm_store::Outbox;

pub(crate) const SEND_UNDONE: &str = "Send undone";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Event {
    Start { late: bool },
    Written(GmailDraftId),
    Unreached,
    Held,
    Accepted(Option<GmailMessageId>),
    Rejected(String),
    Stored(ThreadId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Step {
    Ignore,
    Write(Outbox),
    Sent {
        row: Outbox,
        trash: Option<GmailMessageId>,
        drop_entry: bool,
    },
    Return {
        draft: DraftId,
        kind: NoticeKind,
        message: String,
    },
}

pub(crate) fn step(row: Outbox, event: Event) -> Step {
    let sending = row.state == OutboxState::Sending;
    match event {
        Event::Start { late } if row.state == OutboxState::Waiting => Step::Write(Outbox {
            state: OutboxState::Sending,
            delayed: row.delayed || late,
            gmail_draft_id: None,
            ..row
        }),
        Event::Written(draft) if sending => Step::Write(Outbox {
            gmail_draft_id: Some(draft),
            ..row
        }),
        Event::Unreached | Event::Held if sending && row.undo_requested => Step::Return {
            draft: row.draft,
            kind: NoticeKind::SendUndone,
            message: SEND_UNDONE.to_owned(),
        },
        Event::Unreached if sending => Step::Write(Outbox {
            state: OutboxState::Waiting,
            delayed: true,
            ..row
        }),
        Event::Accepted(message) if sending => accepted(row, message),
        Event::Rejected(message) if sending => Step::Return {
            draft: row.draft,
            kind: NoticeKind::SendFailed,
            message: format!("Couldn't send: {message}"),
        },
        Event::Stored(thread) if row.thread.is_none() => Step::Write(Outbox {
            thread: Some(thread),
            ..row
        }),
        Event::Start { .. }
        | Event::Written(_)
        | Event::Unreached
        | Event::Held
        | Event::Accepted(_)
        | Event::Rejected(_)
        | Event::Stored(_) => Step::Ignore,
    }
}

fn accepted(row: Outbox, message: Option<GmailMessageId>) -> Step {
    let trash = message.clone().filter(|_| row.undo_requested);
    Step::Sent {
        drop_entry: trash.is_some() || row.toast_closed,
        row: Outbox {
            state: OutboxState::Sent,
            sent_message_id: message,
            toast_closed: row.toast_closed || trash.is_some(),
            ..row
        },
        trash,
    }
}

#[cfg(test)]
mod tests {
    use sm_domain::{AccountId, Millis, OutboxId};

    use super::*;

    fn row(state: OutboxState) -> Outbox {
        Outbox {
            id: OutboxId(1),
            account: AccountId(1),
            draft: DraftId::from("d"),
            thread: None,
            send_after: Millis(10_000),
            state,
            gmail_draft_id: Some(GmailDraftId::from("old autosave")),
            sent_message_id: None,
            undo_requested: false,
            toast_closed: false,
            delayed: false,
        }
    }

    fn held(state: OutboxState) -> Outbox {
        Outbox {
            undo_requested: true,
            ..row(state)
        }
    }

    #[test]
    fn only_a_waiting_row_starts_and_it_forgets_older_gmail_drafts() {
        let Step::Write(started) = step(row(OutboxState::Waiting), Event::Start { late: true })
        else {
            panic!("a waiting row starts");
        };
        assert_eq!(started.state, OutboxState::Sending);
        assert!(started.delayed);
        assert_eq!(started.gmail_draft_id, None);
        for state in [OutboxState::Sending, OutboxState::Sent] {
            assert_eq!(step(row(state), Event::Start { late: false }), Step::Ignore);
        }
    }

    #[test]
    fn unreached_goes_back_to_waiting_unless_an_undo_is_held() {
        let Step::Write(back) = step(row(OutboxState::Sending), Event::Unreached) else {
            panic!("an unreached send waits again");
        };
        assert_eq!(back.state, OutboxState::Waiting);
        assert!(back.delayed, "it missed its time");
        assert!(matches!(
            step(held(OutboxState::Sending), Event::Unreached),
            Step::Return {
                kind: NoticeKind::SendUndone,
                ..
            }
        ));
        assert_eq!(
            step(row(OutboxState::Waiting), Event::Unreached),
            Step::Ignore
        );
    }

    #[test]
    fn a_held_undo_stops_the_send_only_when_one_was_asked() {
        assert!(matches!(
            step(held(OutboxState::Sending), Event::Held),
            Step::Return {
                kind: NoticeKind::SendUndone,
                ..
            }
        ));
        assert_eq!(step(row(OutboxState::Sending), Event::Held), Step::Ignore);
    }

    #[test]
    fn accepted_with_a_held_undo_trashes_that_message_and_drops_the_entry() {
        let message = GmailMessageId::from("m1");
        let Step::Sent {
            row: sent,
            trash,
            drop_entry,
        } = step(
            held(OutboxState::Sending),
            Event::Accepted(Some(message.clone())),
        )
        else {
            panic!("an accepted send is sent");
        };
        assert_eq!(sent.state, OutboxState::Sent);
        assert_eq!(sent.sent_message_id, Some(message.clone()));
        assert_eq!(trash, Some(message));
        assert!(drop_entry && sent.toast_closed);
    }

    #[test]
    fn accepted_keeps_the_entry_while_the_toast_shows() {
        let message = GmailMessageId::from("m1");
        let Step::Sent {
            trash, drop_entry, ..
        } = step(
            row(OutboxState::Sending),
            Event::Accepted(Some(message.clone())),
        )
        else {
            panic!("an accepted send is sent");
        };
        assert_eq!(trash, None);
        assert!(!drop_entry, "the toast's Undo can still trash it");
        let closed = Outbox {
            toast_closed: true,
            ..row(OutboxState::Sending)
        };
        let Step::Sent { drop_entry, .. } = step(closed, Event::Accepted(Some(message))) else {
            panic!("an accepted send is sent");
        };
        assert!(drop_entry, "a stray z must not trash delivered mail");
    }

    #[test]
    fn a_rejection_returns_the_draft_with_gmails_reason() {
        assert_eq!(
            step(
                row(OutboxState::Sending),
                Event::Rejected("Invalid To header".to_owned())
            ),
            Step::Return {
                draft: DraftId::from("d"),
                kind: NoticeKind::SendFailed,
                message: "Couldn't send: Invalid To header".to_owned(),
            }
        );
    }

    #[test]
    fn a_stored_message_gives_a_new_messages_row_its_thread_once() {
        let Step::Write(stored) = step(row(OutboxState::Sent), Event::Stored(ThreadId(4))) else {
            panic!("a send with no thread gets one");
        };
        assert_eq!(stored.thread, Some(ThreadId(4)));
        assert_eq!(step(stored, Event::Stored(ThreadId(5))), Step::Ignore);
    }
}
