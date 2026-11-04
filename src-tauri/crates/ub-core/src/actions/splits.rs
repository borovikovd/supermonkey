//! Split Inbox pins and the splits sheet (SPEC Split Inbox).
//!
//! Extension point: the splits slice fills these in, validating rules with
//! `ub_domain::Query::parse_rule` before the store saves them.

use super::ActionError;

/// `pin`: route a sender, domain, or label to a split.
pub(super) fn pin() -> Result<(), ActionError> {
    Err(ActionError::NotYetImplemented("pin"))
}

/// `saveSplits`: the splits sheet.
pub(super) fn save_splits() -> Result<(), ActionError> {
    Err(ActionError::NotYetImplemented("saveSplits"))
}
