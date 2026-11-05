//! Settings: text values by key, global (account 0) or per account. What the
//! keys mean is the caller's.

use std::collections::BTreeMap;

use rusqlite::named_params;
use ub_domain::AccountId;

use crate::StoreError;
use crate::db::Db;
use crate::tx::WriteTx;

/// Reads of settings.
#[derive(Clone, Copy)]
pub struct SettingReads<'c> {
    db: Db<'c>,
}

impl<'c> SettingReads<'c> {
    pub(crate) fn new(db: Db<'c>) -> Self {
        Self { db }
    }

    /// One value; [`AccountId::NONE`] for a global setting.
    pub fn get(self, account: AccountId, key: &str) -> Result<Option<String>, StoreError> {
        self.db.optional(
            "read a setting",
            "SELECT value FROM settings WHERE account_id = ?1 AND key = ?2",
            (account, key),
            |r| r.get("value"),
        )
    }

    /// Every value of one account, or the global ones, by key.
    pub fn all(self, account: AccountId) -> Result<BTreeMap<String, String>, StoreError> {
        let pairs = self.db.all(
            "read settings",
            "SELECT key, value FROM settings WHERE account_id = ?1",
            [account],
            |r| Ok((r.get("key")?, r.get("value")?)),
        )?;
        Ok(pairs.into_iter().collect())
    }
}

/// Writes to settings.
pub struct SettingWrites<'t, 'c> {
    tx: &'t mut WriteTx<'c>,
}

impl<'t, 'c> SettingWrites<'t, 'c> {
    pub(crate) fn new(tx: &'t mut WriteTx<'c>) -> Self {
        Self { tx }
    }

    /// Sets a value; [`AccountId::NONE`] for a global setting.
    pub fn set(&mut self, account: AccountId, key: &str, value: &str) -> Result<(), StoreError> {
        self.tx.db().execute(
            "store a setting",
            "INSERT INTO settings (account_id, key, value) VALUES (:account, :key, :value)
             ON CONFLICT (account_id, key) DO UPDATE SET value = excluded.value",
            named_params! { ":account": account, ":key": key, ":value": value },
        )?;
        self.tx.record().setting(account);
        Ok(())
    }

    /// Removes a value, so its default applies.
    pub fn remove(&mut self, account: AccountId, key: &str) -> Result<(), StoreError> {
        self.tx.db().execute(
            "remove a setting",
            "DELETE FROM settings WHERE account_id = ?1 AND key = ?2",
            (account, key),
        )?;
        self.tx.record().setting(account);
        Ok(())
    }
}
