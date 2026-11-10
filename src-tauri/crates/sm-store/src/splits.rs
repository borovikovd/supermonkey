//! Split Inbox splits and pins (SPEC Split Inbox). Every write re-routes the
//! account's Inbox in the same transaction.

use rusqlite::{Row, named_params};
use sm_domain::{AccountId, Pin, PinKind, Router, Split, SplitId, SplitKind, split_position};

use crate::db::Db;
use crate::routing;
use crate::tx::WriteTx;
use crate::{StoreError, Table};

/// One split.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitRecord {
    /// Its id, never reused.
    pub id: SplitId,
    /// Its account.
    pub account: AccountId,
    /// Its name.
    pub name: String,
    /// Custom, Important, or Other.
    pub kind: SplitKind,
    /// A custom split's rule.
    pub query: String,
    /// Turned on; Other always is.
    pub enabled: bool,
    /// Hidden while empty.
    pub hide_when_empty: bool,
    /// Its place: custom splits, then Important, then Other.
    pub position: i32,
}

/// One split as the settings sheet saves it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitEdit {
    /// Its id; one below 1 is a new custom split.
    pub id: SplitId,
    /// Its name.
    pub name: String,
    /// Its kind; only custom splits can be new.
    pub kind: SplitKind,
    /// A custom split's rule, already checked with `Query::parse_rule`.
    pub query: String,
    /// Turned on.
    pub enabled: bool,
    /// Hidden while empty.
    pub hide_when_empty: bool,
}

fn split_row(r: &Row<'_>) -> rusqlite::Result<SplitRecord> {
    Ok(SplitRecord {
        id: r.get("id")?,
        account: r.get("account_id")?,
        name: r.get("name")?,
        kind: r.get("kind")?,
        query: r.get("query")?,
        enabled: r.get("enabled")?,
        hide_when_empty: r.get("hide_when_empty")?,
        position: r.get("position")?,
    })
}

/// Reads of splits and pins.
#[derive(Debug, Clone, Copy)]
pub struct SplitReads<'c> {
    db: Db<'c>,
}

impl<'c> SplitReads<'c> {
    pub(crate) const fn new(db: Db<'c>) -> Self {
        Self { db }
    }

    /// The account's splits, in order.
    pub fn all(self, account: AccountId) -> Result<Vec<SplitRecord>, StoreError> {
        self.db.all(
            "read the splits",
            "SELECT id, account_id, name, kind, query, enabled, hide_when_empty, position
             FROM splits WHERE account_id = ?1 ORDER BY position, id",
            [account],
            split_row,
        )
    }

    /// One split.
    pub fn get(self, id: SplitId) -> Result<Option<SplitRecord>, StoreError> {
        self.db.optional(
            "read a split",
            "SELECT id, account_id, name, kind, query, enabled, hide_when_empty, position
             FROM splits WHERE id = ?1",
            [id],
            split_row,
        )
    }

    /// The account's pins, by kind and value.
    pub fn pins(self, account: AccountId) -> Result<Vec<Pin>, StoreError> {
        let rows: Vec<(PinKind, String, SplitId)> = self.db.all(
            "read the pins",
            "SELECT kind, value, split_id FROM split_overrides WHERE account_id = ?1
             ORDER BY kind, value",
            [account],
            |r| Ok((r.get("kind")?, r.get("value")?, r.get("split_id")?)),
        )?;
        Ok(rows
            .into_iter()
            .filter_map(|(kind, value, split)| Pin::new(kind, &value, split))
            .collect())
    }

    /// The split a pin of this kind and value sends mail to.
    pub fn pin(
        self,
        account: AccountId,
        kind: PinKind,
        value: &str,
    ) -> Result<Option<SplitId>, StoreError> {
        self.db.optional(
            "read a pin",
            "SELECT split_id FROM split_overrides WHERE account_id = ?1 AND kind = ?2 AND value = ?3",
            (account, kind, value),
            |r| r.get("split_id"),
        )
    }

    /// The account's router over its splits and pins.
    pub fn router(self, account: AccountId) -> Result<Router, StoreError> {
        let splits = self.all(account)?.into_iter().map(|s| Split {
            id: s.id,
            kind: s.kind,
            query: s.query,
            enabled: s.enabled,
        });
        Ok(Router::new(splits.collect::<Vec<_>>(), self.pins(account)?))
    }
}

/// Writes to splits and pins.
#[derive(Debug)]
pub struct SplitWrites<'t, 'c> {
    tx: &'t mut WriteTx<'c>,
}

impl<'t, 'c> SplitWrites<'t, 'c> {
    pub(crate) const fn new(tx: &'t mut WriteTx<'c>) -> Self {
        Self { tx }
    }

    /// The settings sheet's save: deletes the custom splits in `deleted`
    /// (Important and Other can't be), updates saved ones, adds new custom
    /// ones, and places them in the order given: custom splits, then
    /// Important, then Other. Other stays on. Returns each edit's id, new
    /// ones included, in order.
    pub fn save(
        &mut self,
        account: AccountId,
        edits: &[SplitEdit],
        deleted: &[SplitId],
    ) -> Result<Vec<SplitId>, StoreError> {
        let db = self.tx.db();
        for &id in deleted {
            db.execute(
                "delete a split",
                "DELETE FROM splits WHERE id = ?1 AND account_id = ?2 AND kind = 'custom'",
                (id, account),
            )?;
        }
        let mut custom_rank = 0_u32;
        let mut ids = Vec::with_capacity(edits.len());
        for edit in edits {
            let position = split_position(edit.kind, custom_rank);
            if edit.kind == SplitKind::Custom {
                custom_rank = custom_rank.saturating_add(1);
            }
            let name = edit.name.trim();
            let enabled = edit.enabled || edit.kind == SplitKind::Other;
            if edit.id.is_saved() {
                db.execute(
                    "update a split",
                    "UPDATE splits SET name = :name, position = :position, query = :query,
                         enabled = :enabled, hide_when_empty = :hide
                     WHERE id = :id AND account_id = :account AND kind = :kind",
                    named_params! {
                        ":id": edit.id, ":account": account, ":name": name, ":kind": edit.kind,
                        ":position": position, ":query": edit.query, ":enabled": enabled,
                        ":hide": edit.hide_when_empty,
                    },
                )?;
                ids.push(edit.id);
            } else if edit.kind == SplitKind::Custom {
                let id = db.insert(
                    "add a split",
                    "INSERT INTO splits (account_id, name, kind, position, query, enabled,
                         hide_when_empty)
                     VALUES (:account, :name, :kind, :position, :query, :enabled, :hide)",
                    named_params! {
                        ":account": account, ":name": name, ":kind": edit.kind,
                        ":position": position, ":query": edit.query, ":enabled": enabled,
                        ":hide": edit.hide_when_empty,
                    },
                )?;
                ids.push(SplitId(id));
            } else {
                return Err(StoreError::Invalid(format!(
                    "a new split must be custom, not {}",
                    edit.kind
                )));
            }
        }
        self.tx.touch(account, Table::Splits);
        routing::reroute_inbox(self.tx, account)?;
        Ok(ids)
    }

    /// Pins a sender, domain, or label name to a split, replacing any pin
    /// of the same kind and value. Returns the split it replaced.
    pub fn set_pin(
        &mut self,
        account: AccountId,
        pin: &Pin,
    ) -> Result<Option<SplitId>, StoreError> {
        let replaced = self
            .tx
            .read()
            .splits()
            .pin(account, pin.kind, pin.value())?;
        self.tx.db().execute(
            "pin to a split",
            "INSERT INTO split_overrides (account_id, kind, value, split_id)
             VALUES (:account, :kind, :value, :split)
             ON CONFLICT (account_id, kind, value) DO UPDATE SET split_id = excluded.split_id",
            named_params! {
                ":account": account, ":kind": pin.kind, ":value": pin.value(), ":split": pin.split,
            },
        )?;
        self.tx.touch(account, Table::Pins);
        routing::reroute_inbox(self.tx, account)?;
        Ok(replaced)
    }

    /// Removes a pin, as undoing a new one does.
    pub fn remove_pin(
        &mut self,
        account: AccountId,
        kind: PinKind,
        value: &str,
    ) -> Result<(), StoreError> {
        self.tx.db().execute(
            "unpin",
            "DELETE FROM split_overrides WHERE account_id = ?1 AND kind = ?2 AND value = ?3",
            (account, kind, value),
        )?;
        self.tx.touch(account, Table::Pins);
        routing::reroute_inbox(self.tx, account)
    }
}
