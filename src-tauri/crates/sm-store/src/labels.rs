//! Labels, per account (SPEC Labels).

use std::collections::HashMap;

use rusqlite::{Row, named_params};
use sm_domain::{AccountId, LabelId, LabelSet};

use crate::db::{Db, id_list, json_column};
use crate::routing;
use crate::tx::WriteTx;
use crate::{StoreError, Table};

/// One label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelRecord {
    /// The local id: Gmail's, or the UI's for a label made here.
    pub id: LabelId,
    /// Gmail's id, once Gmail has one. Gmail label ids and local ids share
    /// one form, so this is a [`LabelId`] too.
    pub gmail_id: Option<LabelId>,
    /// The name, a path for nested labels.
    pub name: String,
    /// A Gmail system or category label.
    pub system: bool,
}

/// One label as Gmail's `labels.list` gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GmailLabel {
    /// Gmail's id.
    pub gmail_id: LabelId,
    /// The name.
    pub name: String,
    /// Gmail's `type` is `system`.
    pub system: bool,
}

fn label_row(r: &Row<'_>) -> rusqlite::Result<LabelRecord> {
    Ok(LabelRecord {
        id: r.get("id")?,
        gmail_id: r.get("gmail_id")?,
        name: r.get("name")?,
        system: r.get("system")?,
    })
}

/// Reads of labels.
#[derive(Debug, Clone, Copy)]
pub struct LabelReads<'c> {
    db: Db<'c>,
}

impl<'c> LabelReads<'c> {
    pub(crate) const fn new(db: Db<'c>) -> Self {
        Self { db }
    }

    /// The account's labels: system ones first, then by name ignoring case.
    pub fn all(self, account: AccountId) -> Result<Vec<LabelRecord>, StoreError> {
        self.db.all(
            "read the labels",
            "SELECT id, gmail_id, name, system FROM labels WHERE account_id = ?1
             ORDER BY system DESC, name COLLATE NOCASE, id",
            [account],
            label_row,
        )
    }

    /// One label by local id.
    pub fn get(self, account: AccountId, id: &LabelId) -> Result<Option<LabelRecord>, StoreError> {
        self.db.optional(
            "read a label",
            "SELECT id, gmail_id, name, system FROM labels WHERE account_id = ?1 AND id = ?2",
            (account, id),
            label_row,
        )
    }

    /// The label with this name, ignoring case.
    pub fn by_name(
        self,
        account: AccountId,
        name: &str,
    ) -> Result<Option<LabelRecord>, StoreError> {
        self.db.optional(
            "find a label by name",
            "SELECT id, gmail_id, name, system FROM labels
             WHERE account_id = ?1 AND name = ?2 COLLATE NOCASE ORDER BY id LIMIT 1",
            (account, name),
            label_row,
        )
    }

    /// The local id of the label Gmail knows as `gmail_id`.
    pub fn by_gmail_id(
        self,
        account: AccountId,
        gmail_id: &LabelId,
    ) -> Result<Option<LabelId>, StoreError> {
        self.db.optional(
            "find a label by Gmail id",
            "SELECT id FROM labels WHERE account_id = ?1 AND gmail_id = ?2",
            (account, gmail_id),
            |r| r.get("id"),
        )
    }

    /// Local id to Gmail id, for every label Gmail has an id for.
    pub fn gmail_ids(self, account: AccountId) -> Result<HashMap<LabelId, LabelId>, StoreError> {
        let pairs = self.db.all(
            "read the Gmail label ids",
            "SELECT id, gmail_id FROM labels WHERE account_id = ?1 AND gmail_id IS NOT NULL",
            [account],
            |r| Ok((r.get("id")?, r.get("gmail_id")?)),
        )?;
        Ok(pairs.into_iter().collect())
    }

    /// Gmail id to local id, the reverse of [`LabelReads::gmail_ids`], for
    /// mapping a fetched message's labels.
    pub fn local_ids(self, account: AccountId) -> Result<HashMap<LabelId, LabelId>, StoreError> {
        let pairs = self.db.all(
            "read the local label ids",
            "SELECT id, gmail_id FROM labels WHERE account_id = ?1 AND gmail_id IS NOT NULL",
            [account],
            |r| Ok((r.get("gmail_id")?, r.get("id")?)),
        )?;
        Ok(pairs.into_iter().collect())
    }

    /// The names of the account's user labels, by local id: what label pins
    /// and `label:` terms match.
    pub fn user_label_names(
        self,
        account: AccountId,
    ) -> Result<HashMap<LabelId, String>, StoreError> {
        let pairs = self.db.all(
            "read the user label names",
            "SELECT id, name FROM labels WHERE account_id = ?1 AND system = 0",
            [account],
            |r| Ok((r.get("id")?, r.get("name")?)),
        )?;
        Ok(pairs.into_iter().collect())
    }
}

/// Writes to labels. Each records the account's labels as changed, and one
/// that changes a user label's name re-routes the Inbox, since label pins and
/// rules match names.
#[derive(Debug)]
pub struct LabelWrites<'t, 'c> {
    tx: &'t mut WriteTx<'c>,
}

impl<'t, 'c> LabelWrites<'t, 'c> {
    pub(crate) const fn new(tx: &'t mut WriteTx<'c>) -> Self {
        Self { tx }
    }

    /// Adds a user label made here, under the UI's id, until Gmail assigns
    /// its own (SPEC Labels).
    pub fn create_local(
        &mut self,
        account: AccountId,
        id: &LabelId,
        name: &str,
    ) -> Result<(), StoreError> {
        self.changing_names(account, |db| {
            db.execute(
                "add a label",
                "INSERT INTO labels (account_id, id, gmail_id, name, system)
                 VALUES (:account, :id, NULL, :name, 0)",
                named_params! { ":account": account, ":id": id, ":name": name },
            )
            .map(drop)
        })
    }

    /// Replaces the account's Gmail labels with `labels.list`'s answer:
    /// renames known ones, adds new ones under their Gmail id, and drops the
    /// ones Gmail no longer has. Labels made here and not yet on Gmail stay.
    pub fn sync_from_gmail(
        &mut self,
        account: AccountId,
        labels: &[GmailLabel],
    ) -> Result<(), StoreError> {
        self.changing_names(account, |db| {
            for label in labels {
                let renamed = db.execute(
                    "rename a label from Gmail",
                    "UPDATE labels SET name = :name, system = :system
                     WHERE account_id = :account AND gmail_id = :gmail",
                    named_params! {
                        ":account": account, ":gmail": label.gmail_id,
                        ":name": label.name, ":system": label.system,
                    },
                )?;
                if renamed == 0 {
                    db.execute(
                        "add a label from Gmail",
                        "INSERT INTO labels (account_id, id, gmail_id, name, system)
                         VALUES (:account, :gmail, :gmail, :name, :system)
                         ON CONFLICT (account_id, id) DO UPDATE
                         SET gmail_id = excluded.gmail_id, name = excluded.name,
                             system = excluded.system",
                        named_params! {
                            ":account": account, ":gmail": label.gmail_id,
                            ":name": label.name, ":system": label.system,
                        },
                    )?;
                }
            }
            let listed: Vec<&LabelId> = labels.iter().map(|l| &l.gmail_id).collect();
            db.execute(
                "drop labels Gmail no longer has",
                "DELETE FROM labels WHERE account_id = ?1 AND gmail_id IS NOT NULL
                   AND gmail_id NOT IN (SELECT value FROM json_each(?2))",
                (account, id_list(&listed)?),
            )
            .map(drop)
        })
    }

    /// Records the Gmail id of a label made here, once `labels.create`
    /// answers. When another label already has that id (the name existed on
    /// Gmail), the local label merges into it: messages that had the local
    /// label get the other one, and the local label goes. Returns that other
    /// label when the two merged, `None` when `local` keeps its place.
    pub fn assign_gmail_id(
        &mut self,
        account: AccountId,
        local: &LabelId,
        gmail_id: &LabelId,
    ) -> Result<Option<LabelId>, StoreError> {
        let existing: Option<LabelId> = self.tx.db().optional(
            "find a label holding a Gmail id",
            "SELECT id FROM labels WHERE account_id = ?1 AND gmail_id = ?2 AND id != ?3",
            (account, gmail_id, local),
            |r| r.get("id"),
        )?;
        let Some(existing) = existing else {
            self.tx.db().execute(
                "store a label's Gmail id",
                "UPDATE labels SET gmail_id = ?3 WHERE account_id = ?1 AND id = ?2",
                (account, local, gmail_id),
            )?;
            self.tx.touch(account, Table::Labels);
            return Ok(None);
        };
        let carrying: Vec<(sm_domain::MessageId, LabelSet)> = self.tx.db().all(
            "find messages with a label",
            "SELECT m.id, m.label_ids FROM messages m
             WHERE m.account_id = ?1
               AND EXISTS (SELECT 1 FROM json_each(m.label_ids) WHERE value = ?2)",
            (account, local),
            |r| Ok((r.get("id")?, json_column(r, "label_ids")?)),
        )?;
        let relabeled: Vec<_> = carrying
            .into_iter()
            .map(|(id, mut labels)| {
                labels.remove(local.as_str());
                labels.insert(existing.clone());
                (id, labels)
            })
            .collect();
        self.changing_names(account, |db| {
            db.execute(
                "drop a merged label",
                "DELETE FROM labels WHERE account_id = ?1 AND id = ?2",
                (account, local),
            )
            .map(drop)
        })?;
        self.tx.messages().set_labels(&relabeled)?;
        Ok(Some(existing))
    }

    /// Drops a label made here that Gmail refused.
    pub fn delete_local(&mut self, account: AccountId, id: &LabelId) -> Result<(), StoreError> {
        self.changing_names(account, |db| {
            db.execute(
                "drop a refused label",
                "DELETE FROM labels WHERE account_id = ?1 AND id = ?2 AND gmail_id IS NULL",
                (account, id),
            )
            .map(drop)
        })
    }

    /// Runs `write`, records the labels as changed, and re-routes the Inbox
    /// when the user label names changed.
    fn changing_names(
        &mut self,
        account: AccountId,
        write: impl FnOnce(Db<'c>) -> Result<(), StoreError>,
    ) -> Result<(), StoreError> {
        let names = |tx: &WriteTx<'c>| tx.read().labels().user_label_names(account);
        let before = names(self.tx)?;
        write(self.tx.db())?;
        self.tx.touch(account, Table::Labels);
        if names(self.tx)? != before {
            routing::reroute_inbox(self.tx, account)?;
        }
        Ok(())
    }
}
