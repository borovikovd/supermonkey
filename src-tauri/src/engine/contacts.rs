//! Recipient autocomplete. Filled in with the composer.

use rusqlite::Connection;

use crate::model::*;

pub fn suggest(
    _conn: &Connection,
    _account: AccountId,
    _query: &str,
    _limit: usize,
) -> anyhow::Result<Vec<ContactSuggestion>> {
    Ok(Vec::new())
}
