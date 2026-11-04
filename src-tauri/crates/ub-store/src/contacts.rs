//! Contacts for recipient autocomplete (SPEC Recipients and autocomplete).
//! Counting rules and ranking are `ub_domain::contacts`'; this stores the
//! counts and counts each Gmail message once.

use rusqlite::named_params;
use ub_domain::contacts::ContactStats;
use ub_domain::{AccountId, GmailMessageId, Millis};

use crate::StoreError;
use crate::db::Db;
use crate::tx::WriteTx;

/// One address seen on one message, with what it adds to the counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sighting {
    /// The address; stored lowercased.
    pub email: String,
    /// Its display name on this message; empty for none.
    pub name: String,
    /// Messages this account sent to it: 0 or 1.
    pub sent: u32,
    /// Messages this account received from it: 0 or 1.
    pub received: u32,
    /// Seen on list mail.
    pub on_list: bool,
    /// The message's date.
    pub at: Millis,
}

/// A stored contact.
#[derive(Debug, Clone, PartialEq)]
pub struct ContactRecord {
    /// The address, lowercased.
    pub email: String,
    /// The latest display name; empty for none.
    pub name: String,
    /// Its counts, for ranking.
    pub stats: ContactStats,
}

/// Reads of contacts.
#[derive(Clone, Copy)]
pub struct ContactReads<'c> {
    db: Db<'c>,
}

impl<'c> ContactReads<'c> {
    pub(crate) fn new(db: Db<'c>) -> Self {
        Self { db }
    }

    /// Contacts that may match `query`: a superset of what
    /// `contacts::contact_matches` accepts, for it to filter and rank. An
    /// ASCII query narrows by substring; any other gets every contact, since
    /// SQLite folds only ASCII case.
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

    /// Whether a Gmail message was already counted.
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

/// Writes to contacts.
pub struct ContactWrites<'t, 'c> {
    tx: &'t mut WriteTx<'c>,
}

impl<'t, 'c> ContactWrites<'t, 'c> {
    pub(crate) fn new(tx: &'t mut WriteTx<'c>) -> Self {
        Self { tx }
    }

    /// Counts one message's addresses, once per message: false, and no
    /// change, when it was counted before. Each address keeps its latest
    /// name, sums its counts, stays list-only only while every sighting was
    /// on list mail, and keeps its latest contact time.
    pub fn count(
        &mut self,
        account: AccountId,
        message: &GmailMessageId,
        sightings: &[Sighting],
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
        // Autocomplete is a lookup, not a subscription: nothing to patch.
        self.tx.stamp();
        Ok(true)
    }
}
