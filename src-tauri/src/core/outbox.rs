use crate::domain::{GmailDraftId, GmailMessageId, GmailThreadId, OutboxState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OutboxSnapshot {
    pub(crate) state: OutboxState,
    pub(crate) undo_requested: bool,
    pub(crate) toast_closed: bool,
    pub(crate) delayed: bool,
    pub(crate) gmail_draft_id: Option<GmailDraftId>,
    pub(crate) sent_message_id: Option<GmailMessageId>,
}

impl OutboxSnapshot {
    #[cfg(test)]
    #[must_use]
    pub(crate) const fn waiting(gmail_draft_id: Option<GmailDraftId>) -> Self {
        Self {
            state: OutboxState::Waiting,
            undo_requested: false,
            toast_closed: false,
            delayed: false,
            gmail_draft_id,
            sent_message_id: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OutboxEvent {
    Due,
    DeferredOffline,
    UndoRequested,
    ToastClosed,
    DraftWritten(GmailDraftId),
    DraftWriteFailed {
        transient: bool,
        message: String,
    },
    AuthFailed,
    SendSucceeded {
        message_id: GmailMessageId,
        thread_id: GmailThreadId,
    },
    SendRejected(String),
    SendUncertain,
    ProbeDraftPresent,
    ProbeDraftMissing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OutboxCommand {
    None,
    Persist,
    CancelToDraft { notice: Option<OutboxNotice> },
    TrashSentAndDelete { message_id: GmailMessageId },
    DeleteRowAndUndo,
    FetchSentThread { thread_id: GmailThreadId },
    RetrySend,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OutboxNotice {
    Undone,
    Failed(String),
}

#[must_use]
pub(crate) fn step(
    mut next: OutboxSnapshot,
    event: OutboxEvent,
) -> (Option<OutboxSnapshot>, OutboxCommand) {
    match event {
        OutboxEvent::Due => step_due(next),
        OutboxEvent::DeferredOffline => step_deferred(next),
        OutboxEvent::UndoRequested => step_undo(next),
        OutboxEvent::ToastClosed => step_toast_closed(next),
        OutboxEvent::DraftWritten(id) => {
            next.gmail_draft_id = Some(id);
            (Some(next), OutboxCommand::Persist)
        }
        OutboxEvent::DraftWriteFailed { transient, message } => {
            step_draft_write_failed(next, transient, message)
        }
        OutboxEvent::AuthFailed => step_auth_failed(next),
        OutboxEvent::SendSucceeded {
            message_id,
            thread_id,
        } => step_send_succeeded(next, message_id, thread_id),
        OutboxEvent::SendRejected(message) => (
            None,
            OutboxCommand::CancelToDraft {
                notice: Some(OutboxNotice::Failed(message)),
            },
        ),
        OutboxEvent::SendUncertain => {
            if next.state == OutboxState::Sending {
                (Some(next), OutboxCommand::Persist)
            } else {
                (Some(next), OutboxCommand::None)
            }
        }
        OutboxEvent::ProbeDraftPresent => step_probe_present(next),
        OutboxEvent::ProbeDraftMissing => step_probe_missing(next),
    }
}

fn step_due(mut next: OutboxSnapshot) -> (Option<OutboxSnapshot>, OutboxCommand) {
    if next.state != OutboxState::Waiting {
        return (Some(next), OutboxCommand::None);
    }
    if next.undo_requested {
        return (
            None,
            OutboxCommand::CancelToDraft {
                notice: Some(OutboxNotice::Undone),
            },
        );
    }
    next.state = OutboxState::Sending;
    (Some(next), OutboxCommand::Persist)
}

fn step_deferred(mut next: OutboxSnapshot) -> (Option<OutboxSnapshot>, OutboxCommand) {
    if next.state != OutboxState::Waiting || next.delayed {
        return (Some(next), OutboxCommand::None);
    }
    next.delayed = true;
    (Some(next), OutboxCommand::Persist)
}

fn step_undo(mut next: OutboxSnapshot) -> (Option<OutboxSnapshot>, OutboxCommand) {
    match next.state {
        OutboxState::Waiting => (None, OutboxCommand::CancelToDraft { notice: None }),
        OutboxState::Sending => {
            if next.undo_requested {
                (Some(next), OutboxCommand::None)
            } else {
                next.undo_requested = true;
                (Some(next), OutboxCommand::Persist)
            }
        }
        OutboxState::Sent => match next.sent_message_id {
            Some(message_id) => (None, OutboxCommand::TrashSentAndDelete { message_id }),
            None => (None, OutboxCommand::DeleteRowAndUndo),
        },
    }
}

fn step_toast_closed(mut next: OutboxSnapshot) -> (Option<OutboxSnapshot>, OutboxCommand) {
    if next.state == OutboxState::Sent {
        return (None, OutboxCommand::DeleteRowAndUndo);
    }
    if next.toast_closed {
        return (Some(next), OutboxCommand::None);
    }
    next.toast_closed = true;
    (Some(next), OutboxCommand::Persist)
}

fn step_draft_write_failed(
    mut next: OutboxSnapshot,
    transient: bool,
    message: String,
) -> (Option<OutboxSnapshot>, OutboxCommand) {
    if !matches!(next.state, OutboxState::Sending | OutboxState::Waiting) {
        return (Some(next), OutboxCommand::None);
    }
    if !transient {
        return (
            None,
            OutboxCommand::CancelToDraft {
                notice: Some(OutboxNotice::Failed(message)),
            },
        );
    }
    if next.undo_requested {
        return (
            None,
            OutboxCommand::CancelToDraft {
                notice: Some(OutboxNotice::Undone),
            },
        );
    }
    next.state = OutboxState::Waiting;
    next.delayed = true;
    (Some(next), OutboxCommand::Persist)
}

fn step_auth_failed(mut next: OutboxSnapshot) -> (Option<OutboxSnapshot>, OutboxCommand) {
    if next.state == OutboxState::Sent {
        return (Some(next), OutboxCommand::None);
    }
    if next.undo_requested {
        return (
            None,
            OutboxCommand::CancelToDraft {
                notice: Some(OutboxNotice::Undone),
            },
        );
    }
    if next.state == OutboxState::Sending {
        next.state = OutboxState::Waiting;
        next.delayed = true;
        (Some(next), OutboxCommand::Persist)
    } else if next.state == OutboxState::Waiting && !next.delayed {
        next.delayed = true;
        (Some(next), OutboxCommand::Persist)
    } else {
        (Some(next), OutboxCommand::None)
    }
}

fn step_send_succeeded(
    mut next: OutboxSnapshot,
    message_id: GmailMessageId,
    thread_id: GmailThreadId,
) -> (Option<OutboxSnapshot>, OutboxCommand) {
    if next.undo_requested {
        return (None, OutboxCommand::TrashSentAndDelete { message_id });
    }
    if next.toast_closed {
        return (None, OutboxCommand::FetchSentThread { thread_id });
    }
    next.state = OutboxState::Sent;
    next.delayed = false;
    next.sent_message_id = Some(message_id);
    (Some(next), OutboxCommand::FetchSentThread { thread_id })
}

fn step_probe_present(next: OutboxSnapshot) -> (Option<OutboxSnapshot>, OutboxCommand) {
    if next.state != OutboxState::Sending {
        return (Some(next), OutboxCommand::None);
    }
    if next.undo_requested {
        return (
            None,
            OutboxCommand::CancelToDraft {
                notice: Some(OutboxNotice::Undone),
            },
        );
    }
    (Some(next), OutboxCommand::RetrySend)
}

fn step_probe_missing(mut next: OutboxSnapshot) -> (Option<OutboxSnapshot>, OutboxCommand) {
    if next.state != OutboxState::Sending {
        return (Some(next), OutboxCommand::None);
    }
    if next.toast_closed || next.undo_requested {
        return (None, OutboxCommand::DeleteRowAndUndo);
    }
    next.state = OutboxState::Sent;
    next.delayed = false;
    next.sent_message_id = None;
    (Some(next), OutboxCommand::Persist)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mid() -> GmailMessageId {
        GmailMessageId("m1".to_owned())
    }

    fn tid() -> GmailThreadId {
        GmailThreadId("t1".to_owned())
    }

    fn did() -> GmailDraftId {
        GmailDraftId("d1".to_owned())
    }

    #[test]
    fn waiting_transitions_cover_due_defer_undo_and_toast() {
        let w = OutboxSnapshot::waiting(None);
        let (s, cmd) = step(w.clone(), OutboxEvent::DeferredOffline);
        let deferred = s.unwrap();
        assert!(deferred.delayed, "offline sets delayed");
        assert_eq!(cmd, OutboxCommand::Persist);

        let (s, cmd) = step(deferred, OutboxEvent::Due);
        let sending = s.unwrap();
        assert_eq!(sending.state, OutboxState::Sending);
        assert!(sending.delayed, "due preserves delayed until sent");
        assert_eq!(cmd, OutboxCommand::Persist);

        let (s, cmd) = step(w.clone(), OutboxEvent::UndoRequested);
        assert!(s.is_none(), "waiting undo removes outbox row");
        assert_eq!(cmd, OutboxCommand::CancelToDraft { notice: None });

        let (s, cmd) = step(w, OutboxEvent::ToastClosed);
        assert!(s.unwrap().toast_closed, "toast_closed is persisted");
        assert_eq!(cmd, OutboxCommand::Persist);
    }

    #[test]
    fn sending_held_undo_resolves_across_all_outcomes() {
        let sending = OutboxSnapshot {
            state: OutboxState::Sending,
            undo_requested: false,
            toast_closed: false,
            delayed: false,
            gmail_draft_id: Some(did()),
            sent_message_id: None,
        };
        let (held, cmd) = step(sending, OutboxEvent::UndoRequested);
        let held = held.unwrap();
        assert!(held.undo_requested, "undo held while sending");
        assert_eq!(cmd, OutboxCommand::Persist);

        let (s, cmd) = step(
            held.clone(),
            OutboxEvent::SendSucceeded {
                message_id: mid(),
                thread_id: tid(),
            },
        );
        assert!(s.is_none(), "held undo deletes row after send");
        assert_eq!(cmd, OutboxCommand::TrashSentAndDelete { message_id: mid() });

        let (s, cmd) = step(
            held.clone(),
            OutboxEvent::DraftWriteFailed {
                transient: true,
                message: "503".to_owned(),
            },
        );
        assert!(s.is_none(), "held undo cancels on transient write failure");
        assert_eq!(
            cmd,
            OutboxCommand::CancelToDraft {
                notice: Some(OutboxNotice::Undone),
            }
        );

        let (s, cmd) = step(held.clone(), OutboxEvent::AuthFailed);
        assert!(s.is_none(), "held undo cancels on auth failure");
        assert_eq!(
            cmd,
            OutboxCommand::CancelToDraft {
                notice: Some(OutboxNotice::Undone),
            }
        );

        let (s, cmd) = step(held, OutboxEvent::SendRejected("bad".to_owned()));
        assert!(s.is_none(), "rejected send cancels back to draft");
        assert_eq!(
            cmd,
            OutboxCommand::CancelToDraft {
                notice: Some(OutboxNotice::Failed("bad".to_owned())),
            }
        );
    }

    #[test]
    fn uncertain_probes_and_toast_closed_sent_transitions() {
        let mut s = OutboxSnapshot::waiting(Some(did()));
        s.state = OutboxState::Sending;
        let (u, _) = step(s, OutboxEvent::SendUncertain);
        let u = u.unwrap();
        assert_eq!(u.state, OutboxState::Sending);

        let (retry, cmd) = step(u.clone(), OutboxEvent::ProbeDraftPresent);
        assert_eq!(retry.unwrap().state, OutboxState::Sending);
        assert_eq!(cmd, OutboxCommand::RetrySend);

        let (u_held, _) = step(u.clone(), OutboxEvent::UndoRequested);
        let (gone, cmd) = step(u_held.unwrap(), OutboxEvent::ProbeDraftPresent);
        assert!(gone.is_none(), "held undo cancels when probe finds draft");
        assert_eq!(
            cmd,
            OutboxCommand::CancelToDraft {
                notice: Some(OutboxNotice::Undone),
            }
        );

        let (sent_unknown, cmd) = step(u.clone(), OutboxEvent::ProbeDraftMissing);
        let sent_unknown = sent_unknown.unwrap();
        assert_eq!(sent_unknown.state, OutboxState::Sent);
        assert!(
            sent_unknown.sent_message_id.is_none(),
            "missing draft probe leaves sent_message_id unknown"
        );
        assert_eq!(cmd, OutboxCommand::Persist);

        let (closed, cmd) = step(sent_unknown, OutboxEvent::ToastClosed);
        assert!(closed.is_none(), "closing sent toast removes outbox");
        assert_eq!(cmd, OutboxCommand::DeleteRowAndUndo);

        let (u_closed, _) = step(u, OutboxEvent::ToastClosed);
        let (done, cmd) = step(u_closed.unwrap(), OutboxEvent::ProbeDraftMissing);
        assert!(
            done.is_none(),
            "missing probe with closed toast deletes row"
        );
        assert_eq!(cmd, OutboxCommand::DeleteRowAndUndo);
    }
}
