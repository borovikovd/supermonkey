//! Settings: on or off by key, global (account 0) or per account. Stored as
//! the key's text and `on` or `off`.

use std::collections::BTreeMap;

use rusqlite::named_params;
use ub_domain::{AccountId, SettingKey};

use crate::db::Db;
use crate::tx::WriteTx;
use crate::{StoreError, Table};

/// A setting turned on, as stored.
const ON: &str = "on";
/// A setting turned off, as stored.
const OFF: &str = "off";

/// Reads of settings.
#[derive(Clone, Copy)]
pub struct SettingReads<'c> {
    db: Db<'c>,
}

impl<'c> SettingReads<'c> {
    pub(crate) fn new(db: Db<'c>) -> Self {
        Self { db }
    }

    /// Whether a setting is on; [`AccountId::NONE`] for a global one. One
    /// never set has its default.
    pub fn is_on(self, account: AccountId, key: SettingKey) -> Result<bool, StoreError> {
        let value: Option<String> = self.db.optional(
            "read a setting",
            "SELECT value FROM settings WHERE account_id = ?1 AND key = ?2",
            (account, key),
            |r| r.get("value"),
        )?;
        Ok(match value.as_deref() {
            Some(ON) => true,
            Some(OFF) => false,
            _ => key.default_on(),
        })
    }

    /// Every stored value of one account, or the global ones, by key.
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

    /// Turns a setting on or off; [`AccountId::NONE`] for a global one.
    pub fn set(&mut self, account: AccountId, key: SettingKey, on: bool) -> Result<(), StoreError> {
        self.tx.db().execute(
            "store a setting",
            "INSERT INTO settings (account_id, key, value) VALUES (:account, :key, :value)
             ON CONFLICT (account_id, key) DO UPDATE SET value = excluded.value",
            named_params! { ":account": account, ":key": key, ":value": if on { ON } else { OFF } },
        )?;
        self.tx.touch(account, Table::Settings);
        Ok(())
    }
}
