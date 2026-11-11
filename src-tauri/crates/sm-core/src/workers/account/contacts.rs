//! Recipient autocomplete upkeep (SPEC Recipients and autocomplete): what
//! each message adds to the contact counts, and the backfill from the last
//! year of sent mail.

use sm_domain::{Addr, Identity, SystemLabel};
use sm_gmail::{Format, GmailError, Outcome, PageToken, Priority, ThreadQuery};
use sm_store::{NewMessage, Priority as WritePriority, Sighting};

use super::ctx::AccountCtx;
use super::queue::{BACKFILL, Work};
use super::read::{new_message, parse};

/// The backfill's Gmail query.
const BACKFILL_QUERY: &str = "in:sent newer_than:1y";
/// Threads per backfill page.
const BACKFILL_PAGE: u32 = 100;
/// The headers the backfill reads.
const BACKFILL_HEADERS: &[&str] = &["From", "To", "Cc", "Date"];

/// What one message adds to the account's contacts. On mail this account
/// sent, every recipient gains a sent count. On other mail only the sender
/// gains a received count; co-recipients are stored with no count, unless
/// it is list mail, which adds nothing for them. The account's own
/// addresses are never contacts.
pub(crate) fn sightings<'m>(m: &'m NewMessage, me: &Identity) -> Vec<Sighting<'m>> {
    let seen = |a: &'m Addr, sent: u32, received: u32, on_list: bool| {
        let email = a.email.trim();
        (!email.is_empty() && email.contains('@') && !me.is_mine(email)).then(|| Sighting {
            email,
            name: a.name.as_deref().unwrap_or_default(),
            sent,
            received,
            on_list,
            at: m.date,
        })
    };
    if me.is_mine(&m.from.email) {
        m.to.iter()
            .chain(&m.cc)
            .chain(&m.bcc)
            .filter_map(|a| seen(a, 1, 0, false))
            .collect()
    } else {
        let co_recipients = (!m.is_list).then_some(m.to.iter().chain(&m.cc));
        seen(&m.from, 0, 1, m.is_list)
            .into_iter()
            .chain(
                co_recipients
                    .into_iter()
                    .flatten()
                    .filter_map(|a| seen(a, 0, 0, false)),
            )
            .collect()
    }
}

/// One page of the backfill at priority 6: the recipients of each sent
/// message, counted once. The next page queues behind it.
pub(crate) async fn backfill_page(
    ctx: &AccountCtx,
    page: Option<&PageToken>,
) -> Result<(), GmailError> {
    let query = ThreadQuery {
        q: Some(BACKFILL_QUERY),
        page_token: page,
        max_results: Some(BACKFILL_PAGE),
        ..ThreadQuery::default()
    };
    let listed = ctx.gmail.list_threads(Priority::P6, &query).await?;
    for thread in listed.threads {
        let read = ctx
            .gmail
            .get_thread(Priority::P6, &thread.id, Format::Metadata, BACKFILL_HEADERS)
            .await;
        let read = match read {
            Ok(read) => read,
            Err(e) if *e.outcome() == Outcome::NotFound => continue,
            Err(e) => return Err(e),
        };
        let sent: Vec<NewMessage> = read
            .messages
            .into_iter()
            .filter(|m| {
                let has = |l: SystemLabel| m.label_ids.iter().any(|id| id.as_str() == l.id());
                has(SystemLabel::Sent) && !has(SystemLabel::Draft)
            })
            .map(|m| {
                let parsed = parse(&m);
                new_message(m, parsed.headers, &parsed.parts)
            })
            .collect();
        if sent.is_empty() {
            continue;
        }
        let account = ctx.account;
        ctx.writer()
            .queue(WritePriority::Background, "count contacts", move |tx, _| {
                let Some(me) = tx.read().accounts().identity(account)? else {
                    return Ok(());
                };
                for m in &sent {
                    tx.contacts()
                        .count(account, &m.gmail_id, &sightings(m, &me))?;
                }
                Ok(())
            });
    }
    if let Some(next) = listed.next_page_token {
        ctx.queue
            .push(BACKFILL, Work::Backfill { page: Some(next) });
    }
    Ok(())
}
