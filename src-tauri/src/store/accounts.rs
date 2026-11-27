use std::collections::{HashMap, HashSet};

use rusqlite::{Row, params};

use crate::domain::{
    AccountId, ActionNo, DraftId, HistoryId, IMPORTANT_POSITION, Identity, LabelId, NoticeId,
    NoticeKind, OTHER_POSITION, Pin, PinKind, Rev, SendAsAddress, SettingKey, Split, SplitId,
    SplitKind, SyncStatus, SystemLabel, ThreadId, split_position,
};

use super::sql::{id_list, json_col, to_json};
use super::{Db, StoreError, Table, WriteTx};

macro_rules! account_cols {
    () => {
        "id, email, display_name, send_as, position, hue, history_id, sync_status, needs_signin, connected"
    };
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRecord {
    pub id: AccountId,
    pub email: String,
    pub display_name: String,
    pub send_as: Vec<SendAsAddress>,
    pub position: i32,
    pub hue: i64,
    pub history_id: Option<HistoryId>,
    pub sync_status: SyncStatus,
    pub needs_signin: bool,
    pub connected: bool,
}

impl AccountRecord {
    #[must_use]
    pub fn into_identity(self) -> Identity {
        Identity::new(self.email, self.display_name, self.send_as)
    }

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            email: row.get(1)?,
            display_name: row.get(2)?,
            send_as: json_col(row, 3)?,
            position: row.get(4)?,
            hue: row.get(5)?,
            history_id: row.get(6)?,
            sync_status: row.get(7)?,
            needs_signin: row.get::<_, i64>(8)? != 0,
            connected: row.get::<_, i64>(9)? != 0,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelRecord {
    pub id: LabelId,
    pub gmail_id: Option<LabelId>,
    pub name: String,
    pub system: bool,
}

impl LabelRecord {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            gmail_id: row.get(1)?,
            name: row.get(2)?,
            system: row.get::<_, i64>(3)? != 0,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoticeRecord {
    pub id: NoticeId,
    pub account: AccountId,
    pub kind: NoticeKind,
    pub message: String,
    pub draft: Option<DraftId>,
    pub thread: Option<ThreadId>,
}

impl NoticeRecord {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            account: row.get(1)?,
            kind: row.get(2)?,
            message: row.get(3)?,
            draft: row.get(4)?,
            thread: row.get(5)?,
        })
    }
}

impl Db<'_> {
    pub fn rev(self) -> Result<Rev, StoreError> {
        self.one(
            "SELECT rev FROM meta WHERE id = 1",
            [],
            "read meta rev",
            |r| r.get(0),
        )
    }

    pub fn last_action(self) -> Result<ActionNo, StoreError> {
        self.one(
            "SELECT last_action FROM meta WHERE id = 1",
            [],
            "read last_action",
            |r| r.get(0),
        )
    }

    pub fn accounts(self) -> Result<Vec<AccountRecord>, StoreError> {
        self.all(
            concat!(
                "SELECT ",
                account_cols!(),
                " FROM accounts ORDER BY position, id"
            ),
            [],
            "list accounts",
            AccountRecord::from_row,
        )
    }

    pub fn account(self, id: AccountId) -> Result<Option<AccountRecord>, StoreError> {
        self.optional(
            concat!("SELECT ", account_cols!(), " FROM accounts WHERE id = ?1"),
            [id],
            "read account",
            AccountRecord::from_row,
        )
    }

    pub fn account_by_email(self, email: &str) -> Result<Option<AccountRecord>, StoreError> {
        self.optional(
            concat!(
                "SELECT ",
                account_cols!(),
                " FROM accounts WHERE email = ?1"
            ),
            [email],
            "read account by email",
            AccountRecord::from_row,
        )
    }

    pub fn inbox_unread(self, account: AccountId) -> Result<u32, StoreError> {
        let count: i64 = self.one(
            "SELECT COUNT(*) FROM threads WHERE account_id = ?1 AND in_inbox = 1 AND unread = 1",
            [account],
            "count inbox unread",
            |r| r.get(0),
        )?;
        Ok(u32::try_from(count.max(0)).unwrap_or(u32::MAX))
    }

    pub fn labels(self, account: AccountId) -> Result<Vec<LabelRecord>, StoreError> {
        self.all(
            "SELECT id, gmail_id, name, system FROM labels WHERE account_id = ?1 ORDER BY system DESC, name COLLATE NOCASE, id",
            [account],
            "list labels",
            LabelRecord::from_row,
        )
    }

    pub fn label_by_id(
        self,
        account: AccountId,
        id: &LabelId,
    ) -> Result<Option<LabelRecord>, StoreError> {
        self.optional(
            "SELECT id, gmail_id, name, system FROM labels WHERE account_id = ?1 AND id = ?2",
            params![account, id],
            "read label",
            LabelRecord::from_row,
        )
    }

    pub fn label_by_gmail(
        self,
        account: AccountId,
        gmail_id: &LabelId,
    ) -> Result<Option<LabelRecord>, StoreError> {
        self.optional(
            "SELECT id, gmail_id, name, system FROM labels WHERE account_id = ?1 AND gmail_id = ?2",
            params![account, gmail_id],
            "read label by gmail id",
            LabelRecord::from_row,
        )
    }

    pub fn user_label_names(
        self,
        account: AccountId,
    ) -> Result<HashMap<LabelId, String>, StoreError> {
        let rows = self.all(
            "SELECT id, name FROM labels WHERE account_id = ?1 AND system = 0",
            [account],
            "read user label names",
            |r| Ok((r.get::<_, LabelId>(0)?, r.get::<_, String>(1)?)),
        )?;
        Ok(rows.into_iter().collect())
    }

    pub fn splits(self, account: AccountId) -> Result<Vec<Split>, StoreError> {
        self.all(
            "SELECT id, name, kind, position, query, enabled, hide_when_empty FROM splits WHERE account_id = ?1 ORDER BY position, id",
            [account],
            "list splits",
            |r| {
                Ok(Split {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    kind: r.get(2)?,
                    position: r.get(3)?,
                    query: r.get(4)?,
                    enabled: r.get::<_, i64>(5)? != 0,
                    hide_when_empty: r.get::<_, i64>(6)? != 0,
                })
            },
        )
    }

    pub fn pins(self, account: AccountId) -> Result<Vec<Pin>, StoreError> {
        let rows = self.all(
            "SELECT kind, value, split_id FROM split_overrides WHERE account_id = ?1",
            [account],
            "list pins",
            |r| Ok(Pin::new(r.get(0)?, r.get_ref(1)?.as_str()?, r.get(2)?)),
        )?;
        Ok(rows.into_iter().flatten().collect())
    }

    pub fn pin_target(
        self,
        account: AccountId,
        kind: PinKind,
        value: &str,
    ) -> Result<Option<SplitId>, StoreError> {
        self.optional(
            "SELECT split_id FROM split_overrides WHERE account_id = ?1 AND kind = ?2 AND value = ?3",
            params![account, kind, value],
            "read pin target",
            |r| r.get(0),
        )
    }

    pub fn setting_is_on(
        self,
        account: Option<AccountId>,
        key: SettingKey,
    ) -> Result<bool, StoreError> {
        let raw_account = account.map_or(0_i64, |a| a.0);
        let value = self.optional(
            "SELECT value FROM settings WHERE account_id = ?1 AND key = ?2",
            params![raw_account, key],
            "read setting",
            |r| Ok(r.get_ref(0)?.as_str()? == "on"),
        )?;
        let default = matches!(key, SettingKey::RemoteImages);
        Ok(value.unwrap_or(default))
    }

    pub fn notices(self, account: AccountId) -> Result<Vec<NoticeRecord>, StoreError> {
        self.all(
            "SELECT id, account_id, kind, message, draft_id, thread_id FROM notices WHERE account_id = ?1 ORDER BY id",
            [account],
            "list notices",
            NoticeRecord::from_row,
        )
    }
}

#[derive(Debug, Clone, Copy)]
pub struct NewNotice<'a> {
    pub account: AccountId,
    pub kind: NoticeKind,
    pub message: &'a str,
    pub draft: Option<&'a DraftId>,
    pub thread: Option<ThreadId>,
}

impl WriteTx<'_> {
    pub fn set_last_action(&mut self, n: ActionNo) -> Result<(), StoreError> {
        let changed = self.read().execute(
            "UPDATE meta SET last_action = ?1 WHERE id = 1 AND last_action < ?1",
            [n],
            "update last_action",
        )?;
        if changed > 0 {
            self.wrote = true;
        }
        Ok(())
    }

    pub fn create_account(
        &mut self,
        email: &str,
        display_name: &str,
    ) -> Result<AccountRecord, StoreError> {
        let db = self.read();
        let next_pos: i32 = db.one(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM accounts",
            [],
            "next account position",
            |r| r.get(0),
        )?;
        let hue = i64::from(next_pos);
        let id = AccountId(db.insert(
            "INSERT INTO accounts (email, display_name, send_as, position, hue) VALUES (?1, ?2, '[]', ?3, ?4)",
            params![email, display_name, next_pos, hue],
            "insert account",
        )?);
        for sys in SystemLabel::SEEDED {
            db.execute(
                "INSERT INTO labels (account_id, id, gmail_id, name, system) VALUES (?1, ?2, ?2, ?2, 1)",
                params![id, sys.id()],
                "seed system label",
            )?;
        }
        db.execute(
            "INSERT INTO splits (account_id, name, kind, position, query, enabled, hide_when_empty) VALUES (?1, 'Important', ?2, ?3, '', 1, 1)",
            params![id, SplitKind::Important, IMPORTANT_POSITION],
            "seed important split",
        )?;
        db.execute(
            "INSERT INTO splits (account_id, name, kind, position, query, enabled, hide_when_empty) VALUES (?1, 'Other', ?2, ?3, '', 1, 1)",
            params![id, SplitKind::Other, OTHER_POSITION],
            "seed other split",
        )?;
        self.touch(None, Table::Accounts);
        self.touch(Some(id), Table::Accounts);
        self.touch(Some(id), Table::Labels);
        self.touch(Some(id), Table::Splits);
        self.read()
            .account(id)?
            .ok_or(StoreError::NotFound("account"))
    }

    pub fn remove_account(&mut self, account: AccountId) -> Result<Option<String>, StoreError> {
        let Some(existing) = self.read().account(account)? else {
            return Ok(None);
        };
        let db = self.read();
        db.execute(
            "DELETE FROM messages_fts WHERE rowid IN (SELECT id FROM messages WHERE account_id = ?1)",
            [account],
            "delete account fts rows",
        )?;
        db.execute(
            "DELETE FROM settings WHERE account_id = ?1",
            [account],
            "delete account settings",
        )?;
        db.execute(
            "DELETE FROM accounts WHERE id = ?1",
            [account],
            "delete account",
        )?;
        self.invalidate_routing(account);
        self.touch(None, Table::Accounts);
        for table in Table::ALL {
            self.touch(Some(account), table);
        }
        Ok(Some(existing.email))
    }

    pub fn set_profile(
        &mut self,
        account: AccountId,
        display_name: &str,
        send_as: &[SendAsAddress],
    ) -> Result<(), StoreError> {
        let Some(before) = self.read().account(account)? else {
            return Ok(());
        };
        if before.display_name == display_name && before.send_as == send_as {
            return Ok(());
        }
        let encoded = to_json(send_as)?;
        self.read().execute(
            "UPDATE accounts SET display_name = ?2, send_as = ?3 WHERE id = ?1",
            params![account, display_name, encoded],
            "update account profile",
        )?;
        self.touch(None, Table::Accounts);
        self.touch(Some(account), Table::Accounts);
        let emails_before: HashSet<String> = before
            .send_as
            .iter()
            .map(|s| s.email.to_ascii_lowercase())
            .collect();
        let emails_after: HashSet<String> = send_as
            .iter()
            .map(|s| s.email.to_ascii_lowercase())
            .collect();
        if emails_before != emails_after {
            self.invalidate_routing(account);
            self.reroute_inbox(account)?;
        }
        Ok(())
    }

    pub fn set_history(
        &mut self,
        account: AccountId,
        history_id: HistoryId,
    ) -> Result<(), StoreError> {
        let changed = self.read().execute(
            "UPDATE accounts SET history_id = ?2 WHERE id = ?1 AND (history_id IS NULL OR history_id != ?2)",
            params![account, history_id],
            "update history_id",
        )?;
        if changed > 0 {
            self.touch(Some(account), Table::SyncCursor);
        }
        Ok(())
    }

    pub fn clear_history(&mut self, account: AccountId) -> Result<(), StoreError> {
        let changed = self.read().execute(
            "UPDATE accounts SET history_id = NULL WHERE id = ?1 AND history_id IS NOT NULL",
            [account],
            "clear history_id",
        )?;
        if changed > 0 {
            self.touch(Some(account), Table::SyncCursor);
        }
        Ok(())
    }

    pub fn set_sync_status(
        &mut self,
        account: AccountId,
        status: SyncStatus,
    ) -> Result<(), StoreError> {
        let changed = self.read().execute(
            "UPDATE accounts SET sync_status = ?2 WHERE id = ?1 AND sync_status != ?2",
            params![account, status],
            "update sync_status",
        )?;
        if changed > 0 {
            self.touch(None, Table::Accounts);
            self.touch(Some(account), Table::Accounts);
        }
        Ok(())
    }

    pub fn set_needs_signin(&mut self, account: AccountId, needs: bool) -> Result<(), StoreError> {
        let flag = i64::from(needs);
        let changed = self.read().execute(
            "UPDATE accounts SET needs_signin = ?2 WHERE id = ?1 AND needs_signin != ?2",
            params![account, flag],
            "update needs_signin",
        )?;
        if changed > 0 {
            self.touch(None, Table::Accounts);
            self.touch(Some(account), Table::Accounts);
        }
        Ok(())
    }

    pub fn set_connected(&mut self, account: AccountId, connected: bool) -> Result<(), StoreError> {
        let flag = i64::from(connected);
        let changed = self.read().execute(
            "UPDATE accounts SET connected = ?2 WHERE id = ?1 AND connected != ?2",
            params![account, flag],
            "update connected",
        )?;
        if changed > 0 {
            self.touch(Some(account), Table::SyncCursor);
        }
        Ok(())
    }

    pub fn sync_labels(
        &mut self,
        account: AccountId,
        remote: &[(LabelId, String, bool)],
    ) -> Result<(), StoreError> {
        let before = self.read().labels(account)?;
        let before_names = self.read().user_label_names(account)?;
        let remote_ids: HashSet<&LabelId> = remote.iter().map(|(id, _, _)| id).collect();
        let db = self.read();

        for (gmail_id, name, system) in remote {
            let existing = db.label_by_gmail(account, gmail_id)?;
            if let Some(local) = existing {
                db.execute(
                    "UPDATE labels SET name = ?3, system = ?4 WHERE account_id = ?1 AND id = ?2",
                    params![account, local.id, name, i64::from(*system)],
                    "update synced label",
                )?;
            } else {
                db.execute(
                    "INSERT INTO labels (account_id, id, gmail_id, name, system) VALUES (?1, ?2, ?2, ?3, ?4)
                     ON CONFLICT(account_id, id) DO UPDATE SET gmail_id = excluded.gmail_id, name = excluded.name, system = excluded.system",
                    params![account, gmail_id, name, i64::from(*system)],
                    "upsert synced label",
                )?;
            }
        }

        for old in &before {
            if let Some(gid) = &old.gmail_id
                && !old.system
                && !remote_ids.contains(gid)
            {
                db.execute(
                    "DELETE FROM labels WHERE account_id = ?1 AND id = ?2",
                    params![account, old.id],
                    "prune deleted remote label",
                )?;
            }
        }

        let after = self.read().labels(account)?;
        if after != before {
            self.touch(Some(account), Table::Labels);
        }
        let after_names = self.read().user_label_names(account)?;
        if after_names != before_names {
            self.invalidate_routing(account);
            self.reroute_inbox(account)?;
        }
        Ok(())
    }

    pub fn create_local_label(
        &mut self,
        account: AccountId,
        id: &LabelId,
        name: &str,
    ) -> Result<(), StoreError> {
        self.read().execute(
            "INSERT INTO labels (account_id, id, gmail_id, name, system) VALUES (?1, ?2, NULL, ?3, 0)",
            params![account, id, name],
            "insert local label",
        )?;
        self.touch(Some(account), Table::Labels);
        self.invalidate_routing(account);
        self.reroute_inbox(account)?;
        Ok(())
    }

    pub fn assign_gmail_label(
        &mut self,
        account: AccountId,
        local_id: &LabelId,
        gmail_id: &LabelId,
        name: &str,
    ) -> Result<Option<LabelId>, StoreError> {
        let collision = self.read().label_by_gmail(account, gmail_id)?;
        if let Some(collision) = collision
            && collision.id != *local_id
        {
            self.rewrite_label_references(account, local_id, &collision.id)?;
            self.read().execute(
                "DELETE FROM labels WHERE account_id = ?1 AND id = ?2",
                params![account, local_id],
                "remove merged local label",
            )?;
            self.touch(Some(account), Table::Labels);
            self.invalidate_routing(account);
            self.reroute_inbox(account)?;
            return Ok(Some(collision.id));
        }
        self.read().execute(
            "UPDATE labels SET gmail_id = ?3, name = ?4 WHERE account_id = ?1 AND id = ?2",
            params![account, local_id, gmail_id, name],
            "assign gmail label id",
        )?;
        self.touch(Some(account), Table::Labels);
        self.invalidate_routing(account);
        self.reroute_inbox(account)?;
        Ok(None)
    }

    pub fn delete_label(&mut self, account: AccountId, id: &LabelId) -> Result<(), StoreError> {
        let changed = self.read().execute(
            "DELETE FROM labels WHERE account_id = ?1 AND id = ?2",
            params![account, id],
            "delete label",
        )?;
        if changed > 0 {
            self.touch(Some(account), Table::Labels);
            self.invalidate_routing(account);
            self.reroute_inbox(account)?;
        }
        Ok(())
    }

    fn rewrite_label_references(
        &mut self,
        account: AccountId,
        from: &LabelId,
        to: &LabelId,
    ) -> Result<(), StoreError> {
        let rows: Vec<(crate::domain::MessageId, ThreadId, Vec<LabelId>)> = self.read().all(
            "SELECT id, thread_id, label_ids FROM messages WHERE account_id = ?1 AND EXISTS (SELECT 1 FROM json_each(label_ids) WHERE value = ?2)",
            params![account, from],
            "find messages with merged label",
            |r| Ok((r.get(0)?, r.get(1)?, json_col(r, 2)?)),
        )?;
        let mut affected_threads = HashSet::new();
        for (msg_id, thread_id, mut labels) in rows {
            for label in &mut labels {
                if label == from {
                    *label = to.clone();
                }
            }
            labels.sort();
            labels.dedup();
            let rev = self.stamp();
            let encoded = to_json(&labels)?;
            self.read().execute(
                "UPDATE messages SET label_ids = ?2, rev = ?3 WHERE id = ?1",
                params![msg_id, encoded, rev],
                "rewrite message label",
            )?;
            affected_threads.insert(thread_id);
        }
        for tid in affected_threads {
            self.derive_thread(tid)?;
        }
        Ok(())
    }

    pub fn save_splits(
        &mut self,
        account: AccountId,
        splits: &[Split],
        deleted: &[SplitId],
    ) -> Result<(), StoreError> {
        let orphaned_threads = self.delete_splits(account, deleted)?;
        for (idx, split) in splits.iter().enumerate() {
            let pos = split_position(split.kind, i32::try_from(idx).unwrap_or(i32::MAX));
            self.upsert_split(account, split, pos)?;
        }
        self.touch(Some(account), Table::Splits);
        self.invalidate_routing(account);
        self.reroute_inbox(account)?;
        for tid in orphaned_threads {
            let rev = self.stamp();
            self.read().execute(
                "UPDATE threads SET rev = ?2 WHERE id = ?1",
                params![tid, rev],
                "stamp thread from deleted split",
            )?;
            self.record().thread(account, tid);
        }
        Ok(())
    }

    fn delete_splits(
        &mut self,
        account: AccountId,
        deleted: &[SplitId],
    ) -> Result<Vec<ThreadId>, StoreError> {
        if deleted.is_empty() {
            return Ok(Vec::new());
        }
        let del_json = id_list(deleted)?;
        let orphaned = self.read().all(
            "SELECT id FROM threads WHERE account_id = ?1 AND split_id IN (SELECT value FROM json_each(?2))",
            params![account, del_json],
            "find threads in deleted splits",
            |r| r.get::<_, ThreadId>(0),
        )?;
        let removed = self.read().execute(
            "DELETE FROM splits WHERE account_id = ?1 AND kind = ?2 AND id IN (SELECT value FROM json_each(?3))",
            params![account, SplitKind::Custom, del_json],
            "delete custom splits",
        )?;
        if removed > 0 {
            self.touch(Some(account), Table::Pins);
        }
        Ok(orphaned)
    }

    fn upsert_split(&self, account: AccountId, split: &Split, pos: i32) -> Result<(), StoreError> {
        if split.id.is_saved() {
            self.read().execute(
                "UPDATE splits SET name = ?3, position = ?4, query = ?5, enabled = ?6, hide_when_empty = ?7 WHERE account_id = ?1 AND id = ?2",
                params![
                    account,
                    split.id,
                    split.name,
                    pos,
                    split.query,
                    i64::from(split.enabled),
                    i64::from(split.hide_when_empty),
                ],
                "update split",
            )?;
        } else {
            self.read().insert(
                "INSERT INTO splits (account_id, name, kind, position, query, enabled, hide_when_empty) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    account,
                    split.name,
                    SplitKind::Custom,
                    pos,
                    split.query,
                    i64::from(split.enabled),
                    i64::from(split.hide_when_empty),
                ],
                "insert custom split",
            )?;
        }
        Ok(())
    }

    pub fn set_pin(
        &mut self,
        account: AccountId,
        kind: PinKind,
        value: &str,
        split: SplitId,
    ) -> Result<Option<SplitId>, StoreError> {
        let previous = self.read().pin_target(account, kind, value)?;
        self.read().execute(
            "INSERT INTO split_overrides (account_id, kind, value, split_id) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(account_id, kind, value) DO UPDATE SET split_id = excluded.split_id",
            params![account, kind, value, split],
            "upsert pin",
        )?;
        self.touch(Some(account), Table::Pins);
        self.invalidate_routing(account);
        self.reroute_inbox(account)?;
        Ok(previous)
    }

    pub fn remove_pin(
        &mut self,
        account: AccountId,
        kind: PinKind,
        value: &str,
    ) -> Result<(), StoreError> {
        let changed = self.read().execute(
            "DELETE FROM split_overrides WHERE account_id = ?1 AND kind = ?2 AND value = ?3",
            params![account, kind, value],
            "remove pin",
        )?;
        if changed > 0 {
            self.touch(Some(account), Table::Pins);
            self.invalidate_routing(account);
            self.reroute_inbox(account)?;
        }
        Ok(())
    }

    pub fn set_setting(
        &mut self,
        account: Option<AccountId>,
        key: SettingKey,
        on: bool,
    ) -> Result<(), StoreError> {
        let raw_account = account.map_or(0_i64, |a| a.0);
        let value = if on { "on" } else { "off" };
        let current = self.read().setting_is_on(account, key)?;
        if current == on
            && self
                .read()
                .optional(
                    "SELECT 1 FROM settings WHERE account_id = ?1 AND key = ?2",
                    params![raw_account, key],
                    "check setting exists",
                    |r| r.get::<_, i64>(0),
                )?
                .is_some()
        {
            return Ok(());
        }
        self.read().execute(
            "INSERT INTO settings (account_id, key, value) VALUES (?1, ?2, ?3)
             ON CONFLICT(account_id, key) DO UPDATE SET value = excluded.value",
            params![raw_account, key, value],
            "upsert setting",
        )?;
        self.touch(account, Table::Settings);
        if account.is_some() {
            self.touch(None, Table::Accounts);
        }
        Ok(())
    }

    pub fn add_notice(&mut self, notice: &NewNotice<'_>) -> Result<NoticeId, StoreError> {
        let id = NoticeId(self.read().insert(
            "INSERT INTO notices (account_id, kind, message, draft_id, thread_id) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                notice.account,
                notice.kind,
                notice.message,
                notice.draft,
                notice.thread,
            ],
            "insert notice",
        )?);
        self.touch(Some(notice.account), Table::Notices);
        Ok(id)
    }

    pub fn delete_notice(&mut self, id: NoticeId) -> Result<(), StoreError> {
        let account: Option<AccountId> = self.read().optional(
            "SELECT account_id FROM notices WHERE id = ?1",
            [id],
            "find notice account",
            |r| r.get(0),
        )?;
        let Some(account) = account else {
            return Ok(());
        };
        let changed =
            self.read()
                .execute("DELETE FROM notices WHERE id = ?1", [id], "delete notice")?;
        if changed > 0 {
            self.touch(Some(account), Table::Notices);
        }
        Ok(())
    }
}
