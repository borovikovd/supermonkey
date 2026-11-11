//! Accounts: who each one is, its rail place, and its sync state.

use rusqlite::{Row, named_params};
use sm_domain::{
    AccountId, HistoryId, IMPORTANT_POSITION, Identity, Millis, OTHER_POSITION, SendAsAddress,
    SplitKind, SyncStatus, SystemLabel, avatar_hue,
};

use crate::db::{Db, json_column, to_json};
use crate::routing;
use crate::tx::WriteTx;
use crate::{StoreError, Table};

/// One account row, with its inbox unread count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRecord {
    /// Its id.
    pub id: AccountId,
    /// The sign-in address.
    pub email: String,
    /// The display name, from the primary `sendAs` entry.
    pub display_name: String,
    /// Gmail's `sendAs` addresses.
    pub send_as: Vec<SendAsAddress>,
    /// Place on the rail.
    pub position: i32,
    /// The avatar hue slot; [`AccountRecord::hue`] is its color.
    pub hue_slot: i64,
    /// Gmail's history cursor.
    pub history_id: Option<HistoryId>,
    /// When sync last stored a cursor.
    pub last_sync: Option<Millis>,
    /// The stored sync state.
    pub sync_status: SyncStatus,
    /// The refresh token was refused: the account shows Sign in again.
    pub needs_signin: bool,
    /// The connect sync has stored its cursor.
    pub connected: bool,
    /// Unread Inbox threads that are not snoozed.
    pub inbox_unread: u32,
}

impl AccountRecord {
    /// Who the account is, for "mail from me" and From defaults.
    #[must_use]
    pub fn into_identity(self) -> Identity {
        Identity::new(self.email, self.display_name, self.send_as)
    }

    /// The sync state the rail shows: Sign in again overrides the rest.
    #[must_use]
    pub const fn status(&self) -> SyncStatus {
        if self.needs_signin {
            SyncStatus::NeedsSignin
        } else {
            self.sync_status
        }
    }

    /// The avatar color.
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
        last_sync: r.get("last_sync")?,
        sync_status: r.get("sync_status")?,
        needs_signin: r.get("needs_signin")?,
        connected: r.get("connected")?,
        inbox_unread: r.get("inbox_unread")?,
    })
}

/// Reads of accounts.
#[derive(Debug, Clone, Copy)]
pub struct AccountReads<'c> {
    db: Db<'c>,
}

impl<'c> AccountReads<'c> {
    pub(crate) const fn new(db: Db<'c>) -> Self {
        Self { db }
    }

    /// Every account, in rail order.
    pub fn all(self) -> Result<Vec<AccountRecord>, StoreError> {
        self.db.all(
            "read the accounts",
            "SELECT a.id, a.email, a.display_name, a.send_as, a.position, a.hue, a.history_id,
                    a.last_sync, a.sync_status, a.needs_signin, a.connected,
                    (SELECT count(*) FROM threads t
                     WHERE t.account_id = a.id AND t.in_inbox = 1 AND t.unread = 1
                       AND t.snooze_until IS NULL) AS inbox_unread
             FROM accounts a ORDER BY a.position, a.id",
            [],
            account_row,
        )
    }

    /// One account.
    pub fn get(self, id: AccountId) -> Result<Option<AccountRecord>, StoreError> {
        self.db.optional(
            "read an account",
            "SELECT a.id, a.email, a.display_name, a.send_as, a.position, a.hue, a.history_id,
                    a.last_sync, a.sync_status, a.needs_signin, a.connected,
                    (SELECT count(*) FROM threads t
                     WHERE t.account_id = a.id AND t.in_inbox = 1 AND t.unread = 1
                       AND t.snooze_until IS NULL) AS inbox_unread
             FROM accounts a WHERE a.id = ?1",
            [id],
            account_row,
        )
    }

    /// The account signed in as `email`, ignoring case.
    pub fn by_email(self, email: &str) -> Result<Option<AccountId>, StoreError> {
        self.db.optional(
            "find an account by address",
            "SELECT id FROM accounts WHERE email = ?1",
            [email],
            |r| r.get("id"),
        )
    }

    /// Who the account is.
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

/// Writes to accounts.
#[derive(Debug)]
pub struct AccountWrites<'t, 'c> {
    tx: &'t mut WriteTx<'c>,
}

impl<'t, 'c> AccountWrites<'t, 'c> {
    pub(crate) const fn new(tx: &'t mut WriteTx<'c>) -> Self {
        Self { tx }
    }

    /// Adds an account signed in as `email`, last on the rail with the next
    /// hue, with the seeded system labels and the Important and Other
    /// splits (SPEC Split Inbox). Its id is never reused.
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

    /// Removes the account and every row of it: mail, drafts, outbox, ops,
    /// undo history, notices, splits, contacts, and settings. The commit
    /// records every table of the account, but not the ids of its rows.
    pub fn remove(&mut self, id: AccountId) -> Result<(), StoreError> {
        let db = self.tx.db();
        // The index is contentless, so it can't cascade; drop its rows first.
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

    /// Stores the display name and `sendAs` list from Gmail. A change of
    /// addresses re-routes the Inbox, since "from me" decides which message
    /// rules read.
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

    /// Stores Gmail's history cursor and when it was read.
    pub fn set_history(
        &mut self,
        id: AccountId,
        history_id: HistoryId,
        synced_at: Millis,
    ) -> Result<(), StoreError> {
        self.tx.db().execute(
            "store an account's history cursor",
            "UPDATE accounts SET history_id = :history, last_sync = :at WHERE id = :id",
            named_params! { ":id": id, ":history": history_id, ":at": synced_at },
        )?;
        self.tx.touch(id, Table::SyncCursor);
        Ok(())
    }

    /// Marks the connect sync done: its cursor is stored.
    pub fn set_connected(&mut self, id: AccountId) -> Result<(), StoreError> {
        self.tx.db().execute(
            "mark an account connected",
            "UPDATE accounts SET connected = 1 WHERE id = ?1",
            [id],
        )?;
        self.tx.touch(id, Table::SyncCursor);
        Ok(())
    }

    /// Stores the sync state; false, and no write, when it is already that.
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

    /// Sets or clears Sign in again. Clearing it also resets the sync state
    /// to idle.
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
