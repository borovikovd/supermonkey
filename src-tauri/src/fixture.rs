//! Generates the perf fixture (SPEC Performance) and smaller demo data.
//!
//! The perf fixture has two accounts, each with 20,000 cached threads, 2,000
//! of them in Inbox, one 60-message thread, and 5,000 contacts.

use std::path::Path;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use rusqlite::params;

use crate::model::*;
use crate::subs::Subs;
use crate::writer::{Ctx, Touched};

pub struct Size {
    pub threads: usize,
    pub inbox: usize,
    pub contacts: usize,
    pub long_thread: usize,
}

pub const PERF: Size = Size {
    threads: 20_000,
    inbox: 2_000,
    contacts: 5_000,
    long_thread: 60,
};

pub const DEMO: Size = Size {
    threads: 400,
    inbox: 60,
    contacts: 200,
    long_thread: 12,
};

const FIRST: [&str; 24] = [
    "Ada",
    "Grace",
    "Linus",
    "Margaret",
    "Ken",
    "Barbara",
    "Dennis",
    "Frances",
    "Alan",
    "Radia",
    "Edsger",
    "Hedy",
    "Donald",
    "Katherine",
    "John",
    "Shafi",
    "Leslie",
    "Sophie",
    "Tim",
    "Annie",
    "Guido",
    "Jean",
    "Niklaus",
    "Karen",
];
const LAST: [&str; 16] = [
    "Lovelace",
    "Hopper",
    "Torvalds",
    "Hamilton",
    "Thompson",
    "Liskov",
    "Ritchie",
    "Allen",
    "Kay",
    "Perlman",
    "Dijkstra",
    "Lamarr",
    "Knuth",
    "Johnson",
    "McCarthy",
    "Goldwasser",
];
const DOMAINS: [&str; 8] = [
    "example.com",
    "acme.io",
    "initech.com",
    "globex.net",
    "hooli.com",
    "umbrella.org",
    "stark.dev",
    "wayne.co",
];
const SUBJECTS: [&str; 20] = [
    "Q3 planning",
    "Lunch on Thursday?",
    "Contract draft v2",
    "Your receipt",
    "Weekly sync notes",
    "Re: launch checklist",
    "Offsite logistics",
    "Invoice #4821",
    "Design review feedback",
    "Quick question",
    "Board deck",
    "Hiring loop for staff engineer",
    "Travel itinerary",
    "Welcome aboard!",
    "Security update",
    "Budget approval",
    "Customer escalation",
    "Release notes 2.4",
    "Coffee next week",
    "Postmortem: outage",
];
const LINES: [&str; 10] = [
    "Thanks for sending this over — I had a look this morning.",
    "Can we move the call to 3pm? Something came up on my side.",
    "Attached are the numbers we talked about yesterday.",
    "I think we're close. Two small comments inline below.",
    "Let me know if you need anything else from me before Friday.",
    "Looping in the team so everyone has the latest version.",
    "Quick reminder that the deadline is end of week.",
    "Great work on this. The new flow is much faster.",
    "Could you double check the totals on page two?",
    "Sounds good to me, let's go with option B.",
];

fn person(rng: &mut StdRng) -> (String, String) {
    let f = FIRST[rng.random_range(0..FIRST.len())];
    let l = LAST[rng.random_range(0..LAST.len())];
    let d = DOMAINS[rng.random_range(0..DOMAINS.len())];
    (
        format!("{f} {l}"),
        format!("{}.{}@{d}", f.to_lowercase(), l.to_lowercase()),
    )
}

fn body(rng: &mut StdRng) -> (String, String) {
    let n = rng.random_range(1..4);
    let lines: Vec<&str> = (0..n)
        .map(|_| LINES[rng.random_range(0..LINES.len())])
        .collect();
    let html = lines
        .iter()
        .map(|l| format!("<p>{l}</p>"))
        .collect::<String>();
    (html, lines.join(" "))
}

/// Writes fixture data into the database at `path` (created if missing).
pub fn generate(path: &Path, size: &Size, accounts: &[&str]) -> anyhow::Result<()> {
    let mut conn = crate::db::open_writer(path)?;
    let mut rng = StdRng::seed_from_u64(7);
    let now = crate::db::now_ms();
    for (ai, email) in accounts.iter().enumerate() {
        let tx = conn.transaction()?;
        let mut subs = Subs::default();
        let mut ctx = Ctx {
            tx: &tx,
            rev: 1,
            now,
            touched: Touched::default(),
            effects: Vec::new(),
            subs: &mut subs,
            online: false,
        };
        let account = crate::accounts::create(&mut ctx, email)?;
        let name = email.split('@').next().unwrap_or(email).to_string();
        ctx.tx.execute(
            "UPDATE accounts SET display_name = ?2, send_as = ?3, connected = 1, history_id = '1' WHERE id = ?1",
            params![
                account,
                name,
                crate::db::to_json(&vec![SendAs { email: email.to_string(), name: name.clone(), is_default: true, is_primary: true }])
            ],
        )?;
        for (id, lname) in [
            ("Label_1", "Receipts"),
            ("Label_2", "Travel"),
            ("Label_3", "Projects"),
        ] {
            ctx.tx.execute(
                "INSERT OR IGNORE INTO labels (account_id, id, gmail_id, name, system) VALUES (?1, ?2, ?2, ?3, 0)",
                params![account, id, lname],
            )?;
        }

        let mut thread_ids = Vec::with_capacity(size.threads);
        for t in 0..size.threads {
            let gid = format!("t{ai}_{t:06}");
            ctx.tx.execute(
                "INSERT INTO threads (account_id, gmail_id) VALUES (?1, ?2)",
                params![account, gid],
            )?;
            let thread = ctx.tx.last_insert_rowid();
            thread_ids.push(thread);
            let in_inbox = t < size.inbox;
            let count = if t == 3 {
                size.long_thread
            } else {
                rng.random_range(1..5)
            };
            let subject = SUBJECTS[rng.random_range(0..SUBJECTS.len())];
            let (sender_name, sender_email) = person(&mut rng);
            let base = now - (t as i64) * 37 * 60_000 - rng.random_range(0..600_000);
            let category = match rng.random_range(0..10) {
                0..=4 => None,
                5 | 6 => Some("CATEGORY_UPDATES"),
                7 => Some("CATEGORY_PROMOTIONS"),
                8 => Some("CATEGORY_SOCIAL"),
                _ => Some("CATEGORY_PERSONAL"),
            };
            let is_list = matches!(category, Some("CATEGORY_PROMOTIONS" | "CATEGORY_UPDATES"))
                && rng.random_bool(0.6);
            let starred = rng.random_bool(0.03);
            let user_label = (rng.random_bool(0.05))
                .then(|| ["Label_1", "Label_2", "Label_3"][rng.random_range(0..3)]);
            for m in 0..count {
                let from_me = m % 2 == 1 && rng.random_bool(0.7);
                let (fname, femail) = if from_me {
                    (name.clone(), email.to_string())
                } else {
                    (sender_name.clone(), sender_email.clone())
                };
                let to = if from_me {
                    vec![Addr {
                        name: Some(sender_name.clone()),
                        email: sender_email.clone(),
                    }]
                } else {
                    vec![Addr {
                        name: Some(name.clone()),
                        email: email.to_string(),
                    }]
                };
                let mut labels: Vec<&str> = Vec::new();
                if from_me {
                    labels.push("SENT");
                } else {
                    if in_inbox {
                        labels.push("INBOX");
                    }
                    if let Some(c) = category {
                        labels.push(c);
                    }
                    if m + 1 == count && in_inbox && rng.random_bool(0.35) {
                        labels.push("UNREAD");
                    }
                }
                if starred && m + 1 == count {
                    labels.push("STARRED");
                }
                if let Some(l) = user_label {
                    labels.push(l);
                }
                let (html, text) = body(&mut rng);
                let date = base - ((count - 1 - m) as i64) * 3_600_000;
                let subj = if m == 0 {
                    subject.to_string()
                } else {
                    format!("Re: {subject}")
                };
                ctx.tx.execute(
                    "INSERT INTO messages (account_id, thread_id, gmail_id, internal_date, from_name, from_email, to_json,
                        subject, snippet, message_id_hdr, is_list, label_ids, has_body, body_html, body_text)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 1, ?13, ?14)",
                    params![
                        account,
                        thread,
                        format!("m{ai}_{t:06}_{m:02}"),
                        date,
                        fname,
                        femail,
                        crate::db::to_json(&to),
                        subj,
                        text.chars().take(140).collect::<String>(),
                        format!("<m{ai}.{t}.{m}@fixture>"),
                        is_list,
                        crate::db::to_json(&labels),
                        html,
                        text,
                    ],
                )?;
                let mid = ctx.tx.last_insert_rowid();
                ctx.tx.execute(
                    "INSERT INTO messages_fts (rowid, from_text, to_text, cc_text, subject, body) VALUES (?1, ?2, ?3, '', ?4, ?5)",
                    params![mid, format!("{fname} {femail}"), to.iter().map(|a| a.email.clone()).collect::<Vec<_>>().join(" "), subj, text],
                )?;
            }
        }
        for &t in &thread_ids {
            crate::threads::refresh(&mut ctx, t)?;
        }
        for _ in 0..size.contacts {
            let (n, e) = person(&mut rng);
            let e = format!("{}{}", rng.random_range(0..1000), e);
            ctx.tx.execute(
                "INSERT OR IGNORE INTO contacts (account_id, email, name, sent_count, recv_count, list_only, last_contact)
                 VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6)",
                params![account, e, n, rng.random_range(0..20), rng.random_range(0..40), now - rng.random_range(0..31_536_000_000i64)],
            )?;
        }
        drop(ctx);
        tx.commit()?;
    }
    Ok(())
}
