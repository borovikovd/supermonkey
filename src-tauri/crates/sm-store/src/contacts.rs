use rusqlite::named_params;
use sm_domain::contacts::ContactStats;
use sm_domain::{AccountId, GmailMessageId, Millis};

use crate::db::Db;
use crate::tx::WriteTx;
use crate::{StoreError, Table};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sighting<'a> {
    pub email: &'a str,
    pub name: &'a str,
    pub sent: u32,
    pub received: u32,
    pub on_list: bool,
    pub at: Millis,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactRecord {
    pub email: String,
    pub name: String,
    pub stats: ContactStats,
}

#[derive(Debug, Clone, Copy)]
pub struct ContactReads<'c> {
    db: Db<'c>,
}

impl<'c> ContactReads<'c> {
    pub(crate) const fn new(db: Db<'c>) -> Self {
        Self { db }
    }

    pub fn candidates(
        self,
        account: AccountId,
        query: &str,
    ) -> Result<Vec<ContactRecord>, StoreError> {
        let query = query.trim();
        let pattern = if query.is_ascii() {
            let mut p = String::with_capacity(query.len() + 2);
            p.push('%');
            for c in query.chars() {
                if matches!(c, '\\' | '%' | '_') {
                    p.push('\\');
                }
                p.push(c);
            }
            p.push('%');
            p
        } else {
            "%".to_owned()
        };
        self.db.all(
            "read contact candidates",
            "SELECT email, name, sent_count, recv_count, list_only, last_contact FROM contacts
             WHERE account_id = ?1
               AND (email LIKE ?2 ESCAPE '\\' OR name LIKE ?2 ESCAPE '\\')
             ORDER BY email",
            (account, pattern),
            |r| {
                Ok(ContactRecord {
                    email: r.get("email")?,
                    name: r.get("name")?,
                    stats: ContactStats {
                        sent: r.get("sent_count")?,
                        received: r.get("recv_count")?,
                        list_only: r.get("list_only")?,
                        last_contact: r.get("last_contact")?,
                    },
                })
            },
        )
    }

    pub fn is_counted(
        self,
        account: AccountId,
        message: &GmailMessageId,
    ) -> Result<bool, StoreError> {
        self.db.one(
            "check a counted message",
            "SELECT EXISTS (SELECT 1 FROM contact_sources
                            WHERE account_id = ?1 AND gmail_msg_id = ?2) AS counted",
            (account, message),
            |r| r.get("counted"),
        )
    }
}

#[derive(Debug)]
pub struct ContactWrites<'t, 'c> {
    tx: &'t mut WriteTx<'c>,
}

impl<'t, 'c> ContactWrites<'t, 'c> {
    pub(crate) const fn new(tx: &'t mut WriteTx<'c>) -> Self {
        Self { tx }
    }

    pub fn count(
        &mut self,
        account: AccountId,
        message: &GmailMessageId,
        sightings: &[Sighting<'_>],
    ) -> Result<bool, StoreError> {
        let db = self.tx.db();
        let fresh = db.execute(
            "mark a message counted",
            "INSERT OR IGNORE INTO contact_sources (account_id, gmail_msg_id) VALUES (?1, ?2)",
            (account, message),
        )? > 0;
        if !fresh {
            return Ok(false);
        }
        for s in sightings {
            db.execute(
                "count a contact",
                "INSERT INTO contacts (account_id, email, name, sent_count, recv_count, list_only,
                     last_contact)
                 VALUES (:account, :email, :name, :sent, :received, :list_only, :at)
                 ON CONFLICT (account_id, email) DO UPDATE SET
                     name = CASE WHEN excluded.name != ''
                                  AND excluded.last_contact >= contacts.last_contact
                                 THEN excluded.name ELSE contacts.name END,
                     sent_count = contacts.sent_count + excluded.sent_count,
                     recv_count = contacts.recv_count + excluded.recv_count,
                     list_only = min(contacts.list_only, excluded.list_only),
                     last_contact = max(contacts.last_contact, excluded.last_contact)",
                named_params! {
                    ":account": account, ":email": s.email.to_lowercase(), ":name": s.name, ":sent": s.sent,
                    ":received": s.received, ":list_only": s.on_list, ":at": s.at,
                },
            )?;
        }
        self.tx.touch(account, Table::Contacts);
        Ok(true)
    }
}
