use rusqlite::{Row, params};

use crate::domain::contacts::ContactStats;
use crate::domain::{AccountId, Addr, GmailMessageId, Millis};

use super::{Db, StoreError, WriteTx};

#[derive(Debug, Clone)]
pub struct ContactSighting {
    pub gmail_msg_id: GmailMessageId,
    pub date: Millis,
    pub sent_by_me: bool,
    pub is_list: bool,
    pub from: Addr,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
}

impl Db<'_> {
    pub fn contact_candidates(
        self,
        account: AccountId,
        query: &str,
    ) -> Result<Vec<ContactStats>, StoreError> {
        let trimmed = query.trim();
        let rows = if trimmed.is_ascii() && !trimmed.is_empty() {
            let pattern = format!("%{}%", escape_like(&trimmed.to_ascii_lowercase()));
            self.all(
                "SELECT email, name, sent_count, recv_count, list_only, last_contact
                 FROM contacts WHERE account_id = ?1 AND (lower(name) LIKE ?2 ESCAPE '\\' OR lower(email) LIKE ?2 ESCAPE '\\')",
                params![account, pattern],
                "query contacts",
                contact_from_row,
            )?
        } else {
            self.all(
                "SELECT email, name, sent_count, recv_count, list_only, last_contact FROM contacts WHERE account_id = ?1",
                [account],
                "list all contacts",
                contact_from_row,
            )?
        };
        Ok(rows)
    }
}

impl WriteTx<'_> {
    pub fn record_contacts(
        &mut self,
        account: AccountId,
        sightings: &[ContactSighting],
    ) -> Result<(), StoreError> {
        let mut any = false;
        let db = self.read();
        for s in sightings {
            let inserted = db.execute(
                "INSERT OR IGNORE INTO contact_sources (account_id, gmail_msg_id) VALUES (?1, ?2)",
                params![account, s.gmail_msg_id],
                "record contact source",
            )?;
            if inserted == 0 {
                continue;
            }
            any = true;
            if s.sent_by_me {
                for addr in s.to.iter().chain(&s.cc) {
                    upsert_contact(
                        db,
                        ContactDelta {
                            account,
                            addr,
                            sent: 1,
                            recv: 0,
                            is_list: false,
                            date: s.date,
                        },
                    )?;
                }
            } else {
                upsert_contact(
                    db,
                    ContactDelta {
                        account,
                        addr: &s.from,
                        sent: 0,
                        recv: 1,
                        is_list: s.is_list,
                        date: s.date,
                    },
                )?;
                for addr in s.to.iter().chain(&s.cc) {
                    upsert_contact(
                        db,
                        ContactDelta {
                            account,
                            addr,
                            sent: 0,
                            recv: 0,
                            is_list: s.is_list,
                            date: s.date,
                        },
                    )?;
                }
            }
        }
        if any {
            let _ = self.stamp();
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct ContactDelta<'a> {
    account: AccountId,
    addr: &'a Addr,
    sent: i64,
    recv: i64,
    is_list: bool,
    date: Millis,
}

fn upsert_contact(db: Db<'_>, delta: ContactDelta<'_>) -> Result<(), StoreError> {
    let key = delta.addr.key();
    if key.is_empty() || !key.contains('@') {
        return Ok(());
    }
    let name = delta.addr.name.as_deref().unwrap_or("").trim();
    db.execute(
        "INSERT INTO contacts (account_id, email, name, sent_count, recv_count, list_only, last_contact)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(account_id, email) DO UPDATE SET
           name = CASE WHEN excluded.name != '' AND (contacts.name = '' OR excluded.last_contact >= contacts.last_contact) THEN excluded.name ELSE contacts.name END,
           sent_count = contacts.sent_count + excluded.sent_count,
           recv_count = contacts.recv_count + excluded.recv_count,
           list_only = CASE WHEN excluded.list_only = 0 THEN 0 ELSE contacts.list_only END,
           last_contact = MAX(contacts.last_contact, excluded.last_contact)",
        params![
            delta.account,
            key,
            name,
            delta.sent,
            delta.recv,
            i64::from(delta.is_list),
            delta.date,
        ],
        "upsert contact",
    )?;
    Ok(())
}

fn contact_from_row(r: &Row<'_>) -> rusqlite::Result<ContactStats> {
    Ok(ContactStats {
        email: r.get(0)?,
        name: r.get(1)?,
        sent_count: u32::try_from(r.get::<_, i64>(2)?.max(0)).unwrap_or(u32::MAX),
        recv_count: u32::try_from(r.get::<_, i64>(3)?.max(0)).unwrap_or(u32::MAX),
        list_only: r.get::<_, i64>(4)? != 0,
        last_contact: r.get(5)?,
    })
}

fn escape_like(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if matches!(ch, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}
