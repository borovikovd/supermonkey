use rusqlite::named_params;
use sm_domain::{AccountId, SettingKey};

use crate::db::Db;
use crate::tx::WriteTx;
use crate::{StoreError, Table};

const ON: &str = "on";
const OFF: &str = "off";

#[derive(Debug, Clone, Copy)]
pub struct SettingReads<'c> {
    db: Db<'c>,
}

impl<'c> SettingReads<'c> {
    pub(crate) const fn new(db: Db<'c>) -> Self {
        Self { db }
    }

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
}

#[derive(Debug)]
pub struct SettingWrites<'t, 'c> {
    tx: &'t mut WriteTx<'c>,
}

impl<'t, 'c> SettingWrites<'t, 'c> {
    pub(crate) const fn new(tx: &'t mut WriteTx<'c>) -> Self {
        Self { tx }
    }

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
