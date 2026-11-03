//! Debug builds with `--features fake` and `UBERMENSCH_FAKE=1`: an in-process
//! fake Gmail with demo mail and two signed-in accounts, so every flow can be
//! exercised without Google.

use crate::LogErr as _;
use std::sync::{Arc, OnceLock};

use ub_fake_gmail::{FakeGmail, FakeUser, NewAttachment, NewMessage};
use ub_gmail::{MemoryTokenStore, OAuthConfig, QuotaConfig, RefreshToken, TokenStore};

use super::Config;
use crate::writer::Writer;

static FAKE: OnceLock<(FakeGmail, Vec<FakeUser>)> = OnceLock::new();

pub const ACCOUNTS: [&str; 2] = ["alex@demo.dev", "sam@demo.dev"];

const PEOPLE: [(&str, &str); 8] = [
    ("Ada Lovelace", "ada@analytical.io"),
    ("Grace Hopper", "grace@navy.mil"),
    ("Linus Torvalds", "linus@kernel.org"),
    ("Margaret Hamilton", "margaret@apollo.space"),
    ("Ken Thompson", "ken@bell-labs.com"),
    ("Barbara Liskov", "barbara@mit.edu"),
    ("Radia Perlman", "radia@spanning.net"),
    ("Edsger Dijkstra", "edsger@ewd.nl"),
];

const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D, 0x49, 0x48, 0x44, 0x52, 0, 0, 0,
    1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1F, 0x15, 0xC4, 0x89, 0, 0, 0, 0x0D, 0x49, 0x44, 0x41, 0x54,
    0x78, 0x9C, 0x63, 0x60, 0x60, 0xF8, 0x0F, 0, 0x01, 0x04, 0x01, 0x00, 0x5F, 0xB3, 0x7E, 0x3B, 0,
    0, 0, 0, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

#[expect(
    clippy::too_many_lines,
    reason = "Demo mail, a flat list of sample messages."
)]
fn seed(u: &FakeUser, me: &str) {
    let receipts = u.add_label("Receipts");
    let travel = u.add_label("Travel");
    for (i, (name, email)) in PEOPLE.iter().enumerate() {
        let from = format!("{name} <{email}>");
        let first = u.add_message(&NewMessage {
            from: from.clone(),
            to: vec![me.to_string()],
            subject: [
                "Q3 planning",
                "Lunch on Thursday?",
                "Design review",
                "Launch checklist",
            ][i % 4]
                .to_string(),
            html: Some(format!(
                "<p>Hi,</p><p>{}</p><p>Thanks,<br>{}</p>",
                [
                    "Here are the numbers we discussed. Let me know what you think.",
                    "Are you free this week?",
                    "I left comments on the latest mockups.",
                    "Two items left before we ship."
                ][i % 4],
                name.split(' ').next().unwrap_or(name)
            )),
            labels: vec!["INBOX".into(), "UNREAD".into(), "CATEGORY_PERSONAL".into()],
            ..Default::default()
        });
        if i % 2 == 0 {
            // A reply from me, then theirs: a three-message thread.
            let mine = u.add_message(&NewMessage {
                from: me.to_string(),
                to: vec![from.clone()],
                subject: "Re: Q3 planning".into(),
                text: Some("Looks good to me. One question about the second table.".into()),
                labels: vec!["SENT".into()],
                thread_id: Some(first.thread_id.clone()),
                in_reply_to: Some(first.rfc822_message_id),
                ..Default::default()
            });
            u.add_message(&NewMessage {
                from,
                to: vec![me.to_string()],
                cc: vec![PEOPLE[(i + 1) % PEOPLE.len()].1.to_string()],
                subject: "Re: Q3 planning".into(),
                html: Some(
                    "<p>Good catch, fixed. See the chart below.</p><img src=\"cid:chart@demo\" width=\"120\">\
                     <div class=\"gmail_quote\"><blockquote class=\"gmail_quote\">Looks good to me.</blockquote></div>"
                        .into(),
                ),
                labels: vec!["INBOX".into(), "CATEGORY_PERSONAL".into()],
                thread_id: Some(first.thread_id),
                in_reply_to: Some(mine.rfc822_message_id),
                attachments: vec![
                    NewAttachment {
                        filename: "chart.png".into(),
                        mime_type: "image/png".into(),
                        data: PNG.to_vec(),
                        content_id: Some("chart@demo".into()),
                    },
                    NewAttachment {
                        filename: "numbers.csv".into(),
                        mime_type: "text/csv".into(),
                        data: b"quarter,revenue\nQ3,42\n".to_vec(),
                        content_id: None,
                    },
                ],
                ..Default::default()
            });
        }
    }
    for i in 0..6 {
        u.add_message(&NewMessage {
            from: "Weekly Digest <digest@news.example>".into(),
            to: vec![me.to_string()],
            subject: format!("This week in tech #{}", 100 + i),
            html: Some(
                "<h2>Top stories</h2><p>Rust 2027 edition, WebKit news, and more.</p>".into(),
            ),
            labels: vec!["INBOX".into(), "CATEGORY_UPDATES".into()],
            list_id: Some("<digest.news.example>".into()),
            ..Default::default()
        });
    }
    u.add_message(&NewMessage {
        from: "Acme Store <orders@acme.shop>".into(),
        to: vec![me.to_string()],
        subject: "Your receipt #4821".into(),
        text: Some("Thanks for your order. Total: $51.25".into()),
        labels: vec!["INBOX".into(), "CATEGORY_PROMOTIONS".into(), receipts],
        ..Default::default()
    });
    u.add_message(&NewMessage {
        from: "Airline <trips@fly.example>".into(),
        to: vec![me.to_string()],
        subject: "Your trip to Lisbon".into(),
        text: Some("Boarding pass attached.".into()),
        labels: vec!["STARRED".into(), travel],
        ..Default::default()
    });
    u.add_message(&NewMessage {
        from: "Mallory <mallory@evil.example>".into(),
        to: vec![me.to_string()],
        subject: "Totally safe message".into(),
        html: Some(
            "<p onclick=\"alert(1)\">Click <a href=\"javascript:alert(2)\">here</a></p>\
             <script>parent.document.body.innerHTML='pwned'</script><img src=x onerror=alert(3)>\
             <form action=\"https://evil.example\"><input name=p></form><a href=\"https://example.com\">A real link</a>"
                .into(),
        ),
        labels: vec!["INBOX".into(), "UNREAD".into()],
        ..Default::default()
    });
}

/// Starts the fake server and returns the config, token store, and accounts to create.
pub fn setup(writer: &Writer) -> Option<(Config, Arc<dyn TokenStore>)> {
    if !cfg!(debug_assertions) || std::env::var("UBERMENSCH_FAKE").as_deref() != Ok("1") {
        return None;
    }
    let fake = tauri::async_runtime::block_on(async { FakeGmail::start() });
    let store = Arc::new(MemoryTokenStore::default());
    let mut users = Vec::new();
    for email in ACCOUNTS {
        let u = fake.add_user(email);
        u.set_display_name(email.split('@').next().unwrap_or(email));
        seed(&u, email);
        store
            .store_refresh_token(email, &RefreshToken(u.refresh_token().to_owned()))
            .log_err("demo token");
        let created = writer.call_blocking(move |ctx| {
            let known: bool = ctx.tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM accounts WHERE email = ?1)",
                [email],
                |r| r.get(0),
            )?;
            if !known {
                crate::accounts::create(ctx, email)?;
            }
            Ok(())
        });
        created.log_err("demo account");
        users.push(u);
    }
    let config = Config {
        gmail_base: fake.base_url(),
        oauth: OAuthConfig {
            client_id: "demo".into(),
            client_secret: "demo".into(),
            auth_url: fake.auth_url(),
            token_url: fake.token_url(),
            revoke_url: fake.revoke_url(),
        },
        quota: QuotaConfig::default(),
    };
    log::info!("demo mode: fake Gmail at {}", fake.base_url());
    let _ = FAKE.set((fake, users));
    Some((config, store))
}

/// Adds a new inbox message to the first demo account (for polls and notifications).
pub fn deliver(subject: &str) {
    if let Some((_, users)) = FAKE.get()
        && let Some(u) = users.first()
    {
        u.add_message(&NewMessage::inbox(
            "Ada Lovelace <ada@analytical.io>",
            subject,
            "A new message arrived.",
        ));
    }
}
