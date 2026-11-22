use rusqlite::{Row, named_params};
use sm_domain::{
    AccountId, HistoryId, IMPORTANT_POSITION, Identity, OTHER_POSITION, SendAsAddress, SplitKind,
    SyncStatus, SystemLabel, avatar_hue,
};

use crate::db::{Db, json_column, to_json};
use crate::routing;
use crate::tx::WriteTx;
use crate::{StoreError, Table};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRecord {
    pub id: AccountId,
    pub email: String,
    pub display_name: String,
    pub send_as: Vec<SendAsAddress>,
    pub position: i32,
    pub hue_slot: i64,
    pub history_id: Option<HistoryId>,
    pub sync_status: SyncStatus,
    pub needs_signin: bool,
    pub connected: bool,
    pub inbox_unread: u32,
}

impl AccountRecord {
    #[must_use]
    pub fn into_identity(self) -> Identity {
        Identity::new(self.email, self.display_name, self.send_as)
    }

    #[must_use]
    pub const fn status(&self) -> SyncStatus {
        if self.needs_signin {
            SyncStatus::NeedsSignin
        } else {
            self.sync_status
        }
    }

    #[must_use]
    pub fn hue(&self) -> &'static str {
        avatar_hue(self.hue_slot)
    }
}

fn account_row(r: &Row<'_>) -> rusqlite::Result<AccountRecord> {
    Ok(AccountRecord {
        id: r.get("id")?,
        email: r.get("email")?,
        display_name: r.get("display_name")?,
        send_as: json_column(r, "send_as")?,
        position: r.get("position")?,
        hue_slot: r.get("hue")?,
        history_id: r.get("history_id")?,
        sync_status: r.get("sync_status")?,
        needs_signin: r.get("needs_signin")?,
        connected: r.get("connected")?,
        inbox_unread: r.get("inbox_unread")?,
    })
}

#[derive(Debug, Clone, Copy)]
pub struct AccountReads<'c> {
    db: Db<'c>,
}

impl<'c> AccountReads<'c> {
    pub(crate) const fn new(db: Db<'c>) -> Self {
        Self { db }
    }

    pub fn all(self) -> Result<Vec<AccountRecord>, StoreError> {
        self.db.all(
            "read the accounts",
            "SELECT a.id, a.email, a.display_name, a.send_as, a.position, a.hue, a.history_id,
                    a.sync_status, a.needs_signin, a.connected,
                    (SELECT count(*) FROM threads t
                     WHERE t.account_id = a.id AND t.in_inbox = 1 AND t.unread = 1
                       AND t.snooze_until IS NULL) AS inbox_unread
             FROM accounts a ORDER BY a.position, a.id",
            [],
            account_row,
        )
    }

    pub fn get(self, id: AccountId) -> Result<Option<AccountRecord>, StoreError> {
        self.db.optional(
            "read an account",
            "SELECT a.id, a.email, a.display_name, a.send_as, a.position, a.hue, a.history_id,
                    a.sync_status, a.needs_signin, a.connected,
                    (SELECT count(*) FROM threads t
                     WHERE t.account_id = a.id AND t.in_inbox = 1 AND t.unread = 1
                       AND t.snooze_until IS NULL) AS inbox_unread
             FROM accounts a WHERE a.id = ?1",
            [id],
            account_row,
        )
    }

    pub fn by_email(self, email: &str) -> Result<Option<AccountId>, StoreError> {
        self.db.optional(
            "find an account by address",
            "SELECT id FROM accounts WHERE email = ?1",
            [email],
            |r| r.get("id"),
        )
    }

    pub fn identity(self, id: AccountId) -> Result<Option<Identity>, StoreError> {
        self.db.optional(
            "read an account's addresses",
            "SELECT email, display_name, send_as FROM accounts WHERE id = ?1",
            [id],
            |r| {
                Ok(Identity::new(
                    r.get("email")?,
                    r.get("display_name")?,
                    json_column(r, "send_as")?,
                ))
            },
        )
    }
}

#[derive(Debug)]
pub struct AccountWrites<'t, 'c> {
    tx: &'t mut WriteTx<'c>,
}

impl<'t, 'c> AccountWrites<'t, 'c> {
    pub(crate) const fn new(tx: &'t mut WriteTx<'c>) -> Self {
        Self { tx }
    }

    pub fn create(&mut self, email: &str, display_name: &str) -> Result<AccountId, StoreError> {
        let db = self.tx.db();
        let (position, hue): (i32, i64) = db.one(
            "pick a new account's place",
            "SELECT coalesce(max(position), -1) + 1 AS position, coalesce(max(hue), -1) + 1 AS hue
             FROM accounts",
            [],
            |r| Ok((r.get("position")?, r.get("hue")?)),
        )?;
        let id = AccountId(db.insert(
            "add an account",
            "INSERT INTO accounts (email, display_name, position, hue)
             VALUES (:email, :name, :position, :hue)",
            named_params! { ":email": email, ":name": display_name, ":position": position, ":hue": hue },
        )?);
        for (label, name) in SystemLabel::SEEDED {
            db.execute(
                "seed a system label",
                "INSERT INTO labels (account_id, id, gmail_id, name, system)
                 VALUES (:account, :id, :id, :name, 1)",
                named_params! { ":account": id, ":id": label.id(), ":name": name },
            )?;
        }
        for (name, kind, position) in [
            ("Important", SplitKind::Important, IMPORTANT_POSITION),
            ("Other", SplitKind::Other, OTHER_POSITION),
        ] {
            db.execute(
                "add a built-in split",
                "INSERT INTO splits (account_id, name, kind, position, enabled, hide_when_empty)
                 VALUES (:account, :name, :kind, :position, 1, 1)",
                named_params! { ":account": id, ":name": name, ":kind": kind, ":position": position },
            )?;
        }
        for table in [Table::Account, Table::Labels, Table::Splits] {
            self.tx.touch(id, table);
        }
        Ok(id)
    }

    pub fn remove(&mut self, id: AccountId) -> Result<(), StoreError> {
        let db = self.tx.db();
        db.execute(
            "drop a removed account's search index",
            "DELETE FROM messages_fts WHERE rowid IN (SELECT id FROM messages WHERE account_id = ?1)",
            [id],
        )?;
        db.execute(
            "drop a removed account's settings",
            "DELETE FROM settings WHERE account_id = ?1",
            [id],
        )?;
        db.execute(
            "remove an account",
            "DELETE FROM accounts WHERE id = ?1",
            [id],
        )?;
        self.tx.forget_routing(id);
        for table in Table::ALL {
            self.tx.touch(id, table);
        }
        Ok(())
    }

    pub fn set_profile(
        &mut self,
        id: AccountId,
        display_name: &str,
        send_as: &[SendAsAddress],
    ) -> Result<(), StoreError> {
        let before = self.tx.read().accounts().identity(id)?;
        self.tx.db().execute(
            "store an account's profile",
            "UPDATE accounts SET display_name = :name, send_as = :send_as WHERE id = :id",
            named_params! { ":id": id, ":name": display_name, ":send_as": to_json("send_as", send_as)? },
        )?;
        self.tx.touch(id, Table::Account);
        let after = self.tx.read().accounts().identity(id)?;
        if before != after {
            routing::reroute_inbox(self.tx, id)?;
        }
        Ok(())
    }

    pub fn set_history(&mut self, id: AccountId, history_id: HistoryId) -> Result<(), StoreError> {
        self.tx.db().execute(
            "store an account's history cursor",
            "UPDATE accounts SET history_id = :history WHERE id = :id",
            named_params! { ":id": id, ":history": history_id },
        )?;
        self.tx.touch(id, Table::SyncCursor);
        Ok(())
    }

    pub fn set_connected(&mut self, id: AccountId) -> Result<(), StoreError> {
        self.tx.db().execute(
            "mark an account connected",
            "UPDATE accounts SET connected = 1 WHERE id = ?1",
            [id],
        )?;
        self.tx.touch(id, Table::SyncCursor);
        Ok(())
    }

    pub fn set_sync_status(
        &mut self,
        id: AccountId,
        status: SyncStatus,
    ) -> Result<bool, StoreError> {
        let changed = self.tx.db().execute(
            "store an account's sync state",
            "UPDATE accounts SET sync_status = :status WHERE id = :id AND sync_status != :status",
            named_params! { ":id": id, ":status": status },
        )? > 0;
        if changed {
            self.tx.touch(id, Table::Account);
        }
        Ok(changed)
    }

    pub fn set_needs_signin(&mut self, id: AccountId, needs: bool) -> Result<(), StoreError> {
        self.tx.db().execute(
            "store an account's sign-in state",
            "UPDATE accounts
             SET needs_signin = :needs,
                 sync_status = CASE WHEN :needs THEN sync_status ELSE 'idle' END
             WHERE id = :id",
            named_params! { ":id": id, ":needs": needs },
        )?;
        self.tx.touch(id, Table::Account);
        Ok(())
    }
}
