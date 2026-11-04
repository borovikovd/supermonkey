//! Unsubscribe (SPEC: Unsubscribe and block). Reads the latest list message's
//! `List-Unsubscribe` headers from Gmail and uses the best way they offer:
//! a one-click POST, then an email to the list, then the list's page.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use ub_gmail::{Format, Priority, cost};

use super::Engine;
use crate::ids::{AccountId, ThreadId};
use crate::model::Unsubscribed;

const HEADERS: [&str; 2] = ["List-Unsubscribe", "List-Unsubscribe-Post"];

pub async fn unsubscribe(
    e: &Arc<Engine>,
    account: AccountId,
    thread: ThreadId,
) -> anyhow::Result<Unsubscribed> {
    let (owner, msgs) = e.reads.with(|c| {
        Ok((
            crate::threads::account_of(c, thread)?,
            crate::threads::messages(c, thread)?,
        ))
    })?;
    anyhow::ensure!(
        owner == Some(account),
        "thread {thread} is not in account {account}"
    );
    let live = || msgs.iter().rev().filter(|m| m.live());
    let msg = live()
        .find(|m| m.is_list)
        .or_else(|| live().next())
        .context("the conversation has no message to unsubscribe from")?;
    let sender = if msg.from_name.is_empty() {
        msg.from_email.clone()
    } else {
        msg.from_name.clone()
    };
    let rt = e.account(account).context("the account is not signed in")?;

    let res = rt
        .sched
        .run(
            Priority::P1,
            cost::messages_get(Format::Metadata),
            rt.gmail.get_message_headers(&msg.gmail_id, &HEADERS),
        )
        .await;
    e.note(&rt, &res);
    let headers = res.map_err(|err| anyhow::anyhow!("{}", err.outcome))?;
    let Some(header) = headers.header("List-Unsubscribe") else {
        return Ok(Unsubscribed::NoLink { sender });
    };
    let how = ub_mime::list_unsubscribe(header, headers.header("List-Unsubscribe-Post"));

    if how.one_click
        && let Some(url) = how.web.as_deref().filter(|u| may_post(u))
    {
        let posted = e
            .http
            .post(url)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body("List-Unsubscribe=One-Click")
            .timeout(Duration::from_secs(10))
            .send()
            .await;
        match posted {
            Ok(r) if r.status().is_success() => return Ok(Unsubscribed::OneClick { sender }),
            // Fall back to the list's other ways.
            Ok(r) => log::warn!("one-click unsubscribe answered {}", r.status()),
            Err(err) => log::warn!("one-click unsubscribe failed: {}", err.without_url()),
        }
    }
    if let Some(mailto) = how.mailto {
        let from = ub_mime::Addr {
            name: None,
            email: rt.email.clone(),
        };
        let domain = rt.email.rsplit_once('@').map_or("localhost", |(_, d)| d);
        let raw = ub_mime::build_plain(
            &from,
            &ub_mime::Addr {
                name: None,
                email: mailto.to,
            },
            mailto.subject.as_deref().unwrap_or("Unsubscribe"),
            mailto.body.as_deref().unwrap_or("Unsubscribe"),
            crate::clock::now_ms(),
            domain,
        );
        let res = rt
            .sched
            .run(
                Priority::P1,
                cost::MESSAGES_SEND,
                rt.gmail.send_message(&raw),
            )
            .await;
        e.note(&rt, &res);
        res.map_err(|err| anyhow::anyhow!("{}", err.outcome))?;
        return Ok(Unsubscribed::Emailed { sender });
    }
    if let Some(url) = how.web {
        e.platform.open_url(&url);
        return Ok(Unsubscribed::OpenedPage { sender });
    }
    Ok(Unsubscribed::NoLink { sender })
}

/// A one-click POST goes only to `https`. Debug builds also allow loopback
/// `http`, where the E2E server stands in for a list.
fn may_post(url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(url) else {
        return false;
    };
    url.scheme() == "https"
        || (cfg!(debug_assertions)
            && url.scheme() == "http"
            && matches!(url.host_str(), Some("127.0.0.1" | "localhost")))
}
