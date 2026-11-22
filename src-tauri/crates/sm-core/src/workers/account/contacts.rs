use sm_domain::{Addr, Identity, SystemLabel};
use sm_gmail::{Format, GmailError, Message, Outcome, PageToken, Priority, ThreadQuery};
use sm_store::{NewMessage, Priority as WritePriority, Sighting};

use super::ctx::AccountCtx;
use super::queue::{BACKFILL, Work};
use super::read::{new_message, parse};

const BACKFILL_QUERY: &str = "in:sent newer_than:1y";
const BACKFILL_PAGE: u32 = 100;
const BACKFILL_HEADERS: &[&str] = &["From", "To", "Cc", "Date"];

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

fn sent_messages(messages: Vec<Message>) -> Vec<NewMessage> {
    messages
        .into_iter()
        .filter(|m| {
            let has = |l: SystemLabel| m.label_ids.iter().any(|id| id.as_str() == l.id());
            has(SystemLabel::Sent) && !has(SystemLabel::Draft)
        })
        .map(|m| {
            let parsed = parse(&m);
            new_message(m, parsed.headers, &parsed.parts)
        })
        .collect()
}

fn count_sent(ctx: &AccountCtx, sent: Vec<NewMessage>) {
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
        let sent = sent_messages(read.messages);
        if sent.is_empty() {
            continue;
        }
        count_sent(ctx, sent);
    }
    if let Some(next) = listed.next_page_token {
        ctx.queue
            .push(BACKFILL, Work::Backfill { page: Some(next) });
    }
    Ok(())
}
