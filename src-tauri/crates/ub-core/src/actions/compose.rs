//! Drafts in composers (SPEC Compose, Drafts, Send and undo).
//!
//! Extension point: the compose slice fills these in. It also closes the
//! composer subscription with send, discard, and close, and closes open
//! composers on `reset` (see `subscriptions::reset`).

use super::ActionError;

/// `saveDraft`: a fresh composer's first save, or its fields after an edit.
pub(super) fn save_draft() -> Result<(), ActionError> {
    Err(ActionError::NotYetImplemented("saveDraft"))
}

/// `closeComposer`: the untouched rule, and the composer subscription closes.
pub(super) fn close_composer() -> Result<(), ActionError> {
    Err(ActionError::NotYetImplemented("closeComposer"))
}

/// `sendDraft`: queue the draft in the outbox.
pub(super) fn send_draft() -> Result<(), ActionError> {
    Err(ActionError::NotYetImplemented("sendDraft"))
}

/// `discardDraft`: delete the draft, here and on Gmail.
pub(super) fn discard_draft() -> Result<(), ActionError> {
    Err(ActionError::NotYetImplemented("discardDraft"))
}
