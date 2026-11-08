//! Demo mode (`vp run demo`): the app against an in-process fake Gmail with
//! demo mail and two signed-in accounts. Every launch starts from fresh demo
//! data in its own folder, never the user's (AGENTS.md: every way to run the
//! app starts from a known state).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sm_core::{App, GmailConfig};
use sm_fake_gmail::{FakeGmail, FakeUser, NewAttachment, NewMessage};
use sm_gmail::{MemoryTokenStore, OAuthConfig, QuotaConfig, RefreshToken, TokenStore};

/// The demo accounts, in rail order.
const ACCOUNTS: [&str; 2] = ["alex@demo.dev", "sam@demo.dev"];

/// The demo's data folder's name in the system's temporary folder.
const DATA_DIR: &str = "supermonkey-demo";

/// A running demo: the fake Gmail with its seeded users. Keep it for as long
/// as the app runs; dropping it stops the fake.
pub struct Demo {
    fake: FakeGmail,
    tokens: Arc<MemoryTokenStore>,
    data_dir: PathBuf,
}

impl Demo {
    /// Starts the fake Gmail inside the current tokio runtime, seeds the demo
    /// mail, and empties the demo's data folder.
    pub fn start() -> anyhow::Result<Self> {
        let data_dir = std::env::temp_dir().join(DATA_DIR);
        fresh_dir(&data_dir)?;
        let fake = FakeGmail::start();
        let tokens = Arc::new(MemoryTokenStore::default());
        for email in ACCOUNTS {
            let user = fake.add_user(email);
            user.set_display_name(email.split('@').next().unwrap_or(email));
            seed(&user, email);
            tokens.store(email, &RefreshToken::new(user.refresh_token()))?;
        }
        tracing::info!(base = %fake.base_url(), "demo mode: fake Gmail started");
        Ok(Self {
            fake,
            tokens,
            data_dir,
        })
    }

    /// The demo's data folder, empty at start.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// The fake's endpoints, with Google's request budget.
    pub fn gmail_config(&self) -> GmailConfig {
        GmailConfig {
            base_url: self.fake.base_url(),
            oauth: OAuthConfig {
                client_id: "demo".into(),
                client_secret: "demo".into(),
                auth_url: self.fake.auth_url(),
                token_url: self.fake.token_url(),
                revoke_url: self.fake.revoke_url(),
            },
            quota: QuotaConfig::default(),
        }
    }

    /// The demo accounts' refresh tokens, in memory.
    pub fn tokens(&self) -> Arc<dyn TokenStore> {
        let tokens: Arc<dyn TokenStore> = Arc::<MemoryTokenStore>::clone(&self.tokens);
        tokens
    }

    /// Signs the demo accounts in on `app`, before it launches.
    pub async fn register(&self, app: &App) -> anyhow::Result<()> {
        for email in ACCOUNTS {
            app.accounts().register(email).await?;
        }
        Ok(())
    }
}

/// Empties the demo's data folder, or makes it. Only ever this folder.
fn fresh_dir(dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    std::fs::create_dir_all(dir)
}

/// People who write to the demo accounts.
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

const SUBJECTS: [&str; 4] = [
    "Q3 planning",
    "Lunch on Thursday?",
    "Design review",
    "Launch checklist",
];

const LINES: [&str; 4] = [
    "Here are the numbers we discussed. Let me know what you think.",
    "Are you free this week?",
    "I left comments on the latest mockups.",
    "Two items left before we ship.",
];

/// A one-pixel PNG, for an inline image.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D, 0x49, 0x48, 0x44, 0x52, 0, 0, 0,
    1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1F, 0x15, 0xC4, 0x89, 0, 0, 0, 0x0D, 0x49, 0x44, 0x41, 0x54,
    0x78, 0x9C, 0x63, 0x60, 0x60, 0xF8, 0x0F, 0, 0x01, 0x04, 0x01, 0x00, 0x5F, 0xB3, 0x7E, 0x3B, 0,
    0, 0, 0, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

fn labels(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|&l| l.to_owned()).collect()
}

/// The demo mail of the account `me`.
fn seed(user: &FakeUser, me: &str) {
    seed_people(user, me);
    seed_the_rest(user, me);
}

/// Conversations with people, every other one with a reply from `me`
/// and an answer with an inline image and attachments.
fn seed_people(user: &FakeUser, me: &str) {
    for (i, (name, email)) in PEOPLE.iter().enumerate() {
        let from = format!("{name} <{email}>");
        let subject = SUBJECTS
            .get(i % SUBJECTS.len())
            .copied()
            .unwrap_or_default();
        let line = LINES.get(i % LINES.len()).copied().unwrap_or_default();
        let first_name = name.split(' ').next().unwrap_or(name);
        let first = user.add_message(&NewMessage {
            from: from.clone(),
            to: vec![me.to_owned()],
            subject: subject.to_owned(),
            html: Some(format!(
                "<p>Hi,</p><p>{line}</p><p>Thanks,<br>{first_name}</p>"
            )),
            labels: labels(&["INBOX", "UNREAD", "CATEGORY_PERSONAL"]),
            ..Default::default()
        });
        if i % 2 != 0 {
            continue;
        }
        // A reply from the account, then theirs: a three-message thread.
        let reply_subject = format!("Re: {subject}");
        let mine = user.add_message(&NewMessage {
            from: me.to_owned(),
            to: vec![from.clone()],
            subject: reply_subject.clone(),
            text: Some("Looks good to me. One question about the second table.".into()),
            labels: labels(&["SENT"]),
            thread_id: Some(first.thread_id.clone()),
            in_reply_to: Some(first.rfc822_message_id),
            ..Default::default()
        });
        let cc = PEOPLE
            .get((i + 1) % PEOPLE.len())
            .map(|(_, e)| (*e).to_owned());
        user.add_message(&NewMessage {
            from,
            to: vec![me.to_owned()],
            cc: cc.into_iter().collect(),
            subject: reply_subject,
            html: Some(
                "<p>Good catch, fixed. See the chart below.</p>\
                 <img src=\"cid:chart@demo\" width=\"120\">\
                 <div class=\"gmail_quote\"><blockquote class=\"gmail_quote\">\
                 Looks good to me.</blockquote></div>"
                    .into(),
            ),
            labels: labels(&["INBOX", "CATEGORY_PERSONAL"]),
            thread_id: Some(first.thread_id),
            in_reply_to: Some(mine.rfc822_message_id),
            attachments: vec![
                NewAttachment {
                    filename: "chart.png".into(),
                    mime_type: "image/png".into(),
                    data: PNG.to_vec(),
                    content_id: Some("chart@demo".into()),
                    inline: false,
                    served_inline: false,
                },
                NewAttachment {
                    filename: "numbers.csv".into(),
                    mime_type: "text/csv".into(),
                    data: b"quarter,revenue\nQ3,42\n".to_vec(),
                    content_id: None,
                    inline: false,
                    served_inline: false,
                },
            ],
            ..Default::default()
        });
    }
}

/// Newsletters, a receipt and a trip under user labels, and a hostile
/// message the sanitizer must defuse.
fn seed_the_rest(user: &FakeUser, me: &str) {
    let receipts = user.add_label("Receipts");
    let travel = user.add_label("Travel");
    for issue in 100..106 {
        user.add_message(&NewMessage {
            from: "Weekly Digest <digest@news.example>".into(),
            to: vec![me.to_owned()],
            subject: format!("This week in tech #{issue}"),
            html: Some(
                "<h2>Top stories</h2><p>Rust 2027 edition, WebKit news, and more.</p>".into(),
            ),
            labels: labels(&["INBOX", "CATEGORY_UPDATES"]),
            list_id: Some("<digest.news.example>".into()),
            extra_headers: vec![(
                "List-Unsubscribe".into(),
                "<https://news.example/unsubscribe>".into(),
            )],
            ..Default::default()
        });
    }
    user.add_message(&NewMessage {
        from: "Acme Store <orders@acme.shop>".into(),
        to: vec![me.to_owned()],
        subject: "Your receipt #4821".into(),
        text: Some("Thanks for your order. Total: $51.25".into()),
        labels: vec!["INBOX".into(), "CATEGORY_PROMOTIONS".into(), receipts],
        ..Default::default()
    });
    user.add_message(&NewMessage {
        from: "Airline <trips@fly.example>".into(),
        to: vec![me.to_owned()],
        subject: "Your trip to Lisbon".into(),
        text: Some("Boarding pass attached.".into()),
        labels: vec!["STARRED".into(), travel],
        ..Default::default()
    });
    user.add_message(&NewMessage {
        from: "Mallory <mallory@evil.example>".into(),
        to: vec![me.to_owned()],
        subject: "Totally safe message".into(),
        html: Some(
            "<p onclick=\"alert(1)\">Click <a href=\"javascript:alert(2)\">here</a></p>\
             <script>parent.document.body.innerHTML='pwned'</script>\
             <img src=x onerror=alert(3)>\
             <form action=\"https://evil.example\"><input name=p></form>\
             <a href=\"https://example.com\">A real link</a>"
                .into(),
        ),
        labels: labels(&["INBOX", "UNREAD"]),
        ..Default::default()
    });
}
