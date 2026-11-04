//! Mail actions: archive, trash, spam, star, read state, snooze, labels, and
//! undo (SPEC Archive, trash, star, read; Labels; Snooze; Undo).
//!
//! Extension point: the actions slice fills these in, over
//! `ub_domain::plan_modify` and the store's op queue and undo history.

use super::ActionError;

/// `modify`: change threads' labels or snooze.
pub(super) fn modify() -> Result<(), ActionError> {
    Err(ActionError::NotYetImplemented("modify"))
}

/// `undo`: revert an undo history entry.
pub(super) fn undo() -> Result<(), ActionError> {
    Err(ActionError::NotYetImplemented("undo"))
}

/// `createLabel`: a label with a UI-made id, applied in the same action.
pub(super) fn create_label() -> Result<(), ActionError> {
    Err(ActionError::NotYetImplemented("createLabel"))
}
