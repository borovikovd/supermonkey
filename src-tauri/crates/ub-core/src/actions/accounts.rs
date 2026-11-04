//! Removing an account (SPEC Settings).
//!
//! Extension point: the accounts slice fills this in: it stops the
//! account's worker, deletes its rows and token, and closes its
//! subscriptions (the commit's `Changes::accounts`).

use super::ActionError;

/// `removeAccount`: sign out and forget an account.
pub(super) fn remove_account() -> Result<(), ActionError> {
    Err(ActionError::NotYetImplemented("removeAccount"))
}
