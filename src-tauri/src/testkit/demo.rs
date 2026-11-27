use std::path::PathBuf;

use sm_fake_gmail::{FakeGmail, NewAttachment, NewMessage};
use tempfile::TempDir;

use crate::core::{Clock, CoreConfig};
use crate::gmail::{OAuthConfig, QuotaConfig, RefreshToken, TokenFile, Units};
use crate::store::with_write_tx;

pub struct Demo {
    dir: TempDir,
    fake: FakeGmail,
}

impl std::fmt::Debug for Demo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Demo")
            .field("dir", &self.dir.path())
            .finish_non_exhaustive()
    }
}

impl Demo {
    pub fn start() -> std::io::Result<Self> {
        let dir = TempDir::new()?;
        let fake = FakeGmail::start();
        seed_demo_world(&fake, dir.path())?;
        Ok(Self { dir, fake })
    }

    #[must_use]
    pub fn core_config(&self) -> CoreConfig {
        CoreConfig {
            data_dir: self.dir.path().to_path_buf(),
            gmail_base_url: self.fake.base_url(),
            oauth: OAuthConfig {
                client_id: "demo-client".to_owned(),
                client_secret: "demo-secret".to_owned(),
                auth_url: self.fake.auth_url(),
                token_url: self.fake.token_url(),
                revoke_url: self.fake.revoke_url(),
            },
            quota: QuotaConfig::for_budget(Units(60_000)),
            clock: Clock::system(),
        }
    }

    #[must_use]
    pub fn data_dir(&self) -> PathBuf {
        self.dir.path().to_path_buf()
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "seeds two realistic demo accounts, labels, sample conversations, and a draft"
)]
fn seed_demo_world(fake: &FakeGmail, data_dir: &std::path::Path) -> std::io::Result<()> {
    let ana = fake.add_user("ana@acme.test");
    ana.set_display_name("Ana Ames");
    ana.add_send_as("ana@ames.dev".to_owned(), "Ana Personal".to_owned());
    drop(ana.add_label("Work"));
    drop(ana.add_label("Receipts"));
    drop(ana.add_label("Travel"));

    let ben = fake.add_user("ben@bolt.test");
    ben.set_display_name("Ben Bolt");
    drop(ben.add_label("Projects"));

    let tokens = TokenFile::new(data_dir.join("tokens.json"));
    tokens
        .store(ana.email(), RefreshToken::new(ana.refresh_token()))
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    tokens
        .store(ben.email(), RefreshToken::new(ben.refresh_token()))
        .map_err(|e| std::io::Error::other(e.to_string()))?;

    with_write_tx(&data_dir.join("mail.db"), |wtx| {
        let _ = wtx.create_account("ana@acme.test", "Ana Ames")?;
        let _ = wtx.create_account("ben@bolt.test", "Ben Bolt")?;
        Ok(())
    })
    .map_err(|e| std::io::Error::other(e.to_string()))?;

    let now_ms = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0);
    let min = 60_000_u64;

    let plan = ana.add_message(NewMessage {
        from: "Carol Chen <carol@corp.test>".to_owned(),
        to: vec!["ana@acme.test".to_owned(), "dave@corp.test".to_owned()],
        subject: "Q4 Roadmap Review".to_owned(),
        html: Some(
            "<p>Hey Ana,</p><p>Attached is the latest Q4 roadmap draft. Let me know if the milestones look realistic before Friday.</p>".to_owned(),
        ),
        labels: vec![
            "INBOX".to_owned(),
            "UNREAD".to_owned(),
            "CATEGORY_PERSONAL".to_owned(),
            "STARRED".to_owned(),
        ],
        attachments: vec![NewAttachment {
            filename: "q4-roadmap.txt".to_owned(),
            mime_type: "text/plain".to_owned(),
            data: b"Q4 Roadmap\n- Fast local SQLite sync\n- Keyboard triage\n".to_vec(),
            ..Default::default()
        }],
        internal_date: Some(now_ms.saturating_sub(15 * min)),
        ..Default::default()
    });
    drop(ana.add_message(NewMessage {
        from: "Dave Diaz <dave@corp.test>".to_owned(),
        to: vec!["ana@acme.test".to_owned(), "carol@corp.test".to_owned()],
        subject: "Re: Q4 Roadmap Review".to_owned(),
        text: Some("Looks solid from engineering — two minor notes on timeline.".to_owned()),
        labels: vec![
            "INBOX".to_owned(),
            "UNREAD".to_owned(),
            "CATEGORY_PERSONAL".to_owned(),
        ],
        thread_id: Some(plan.thread_id),
        in_reply_to: Some(plan.rfc822_message_id),
        internal_date: Some(now_ms.saturating_sub(5 * min)),
        ..Default::default()
    }));

    drop(ana.add_message(NewMessage {
        from: "Pia Park <pia@people.test>".to_owned(),
        to: vec!["ana@acme.test".to_owned()],
        subject: "Coffee this Thursday?".to_owned(),
        text: Some("Are you free around 10:30 near the park?".to_owned()),
        labels: vec![
            "INBOX".to_owned(),
            "UNREAD".to_owned(),
            "CATEGORY_PERSONAL".to_owned(),
        ],
        internal_date: Some(now_ms.saturating_sub(45 * min)),
        ..Default::default()
    }));

    drop(ana.add_message(NewMessage {
        from: "Weekly Digest <news@updates.test>".to_owned(),
        to: vec!["ana@acme.test".to_owned()],
        subject: "This week in Rust & Tauri".to_owned(),
        html: Some("<p>Top picks from the community this week.</p>".to_owned()),
        labels: vec!["INBOX".to_owned(), "CATEGORY_UPDATES".to_owned()],
        list_id: Some("<weekly.updates.test>".to_owned()),
        internal_date: Some(now_ms.saturating_sub(180 * min)),
        ..Default::default()
    }));

    drop(ana.add_draft(NewMessage {
        from: "Ana Ames <ana@acme.test>".to_owned(),
        to: vec!["pia@people.test".to_owned()],
        subject: "Re: Coffee this Thursday?".to_owned(),
        html: Some("<p>Thursday at 10:30 works great!</p>".to_owned()),
        internal_date: Some(now_ms.saturating_sub(2 * min)),
        ..Default::default()
    }));

    drop(ben.add_message(NewMessage {
        from: "Ops Alerts <ops@bolt.test>".to_owned(),
        to: vec!["ben@bolt.test".to_owned()],
        subject: "Deployment completed".to_owned(),
        text: Some("Release 0.7.1 rolled out cleanly with zero errors.".to_owned()),
        labels: vec![
            "INBOX".to_owned(),
            "UNREAD".to_owned(),
            "CATEGORY_PERSONAL".to_owned(),
        ],
        internal_date: Some(now_ms.saturating_sub(20 * min)),
        ..Default::default()
    }));

    Ok(())
}
