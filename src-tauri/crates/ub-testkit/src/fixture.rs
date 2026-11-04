//! The perf fixture (SPEC Performance), and smaller demo-sized data: a
//! database written through the store, as sync would leave it. The perf
//! fixture has two accounts, each with 20,000 cached threads, 2,000 of them
//! in Inbox, one 60-message thread, and 5,000 contacts.

use std::path::Path;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use ub_domain::{
    AccountId, Addr, GmailMessageId, GmailThreadId, LabelId, LabelSet, Millis, SendAsAddress,
};
use ub_store::{
    FetchedThread, GmailLabel, NewBody, NewMessage, Priority, Sighting, StoreError, WriteTx, Writer,
};

/// How much data to write per account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    /// Cached threads.
    pub threads: usize,
    /// Of them, in Inbox.
    pub inbox: usize,
    /// Contacts for autocomplete.
    pub contacts: usize,
    /// Messages in the one long thread.
    pub long_thread: usize,
}

/// The fixture SPEC Performance measures against.
pub const PERF: Size = Size {
    threads: 20_000,
    inbox: 2_000,
    contacts: 5_000,
    long_thread: 60,
};

/// A small fixture, for a quick look.
pub const DEMO: Size = Size {
    threads: 400,
    inbox: 60,
    contacts: 200,
    long_thread: 12,
};

/// The fixture's user labels: Gmail's id and the name.
const LABELS: [(&str, &str); 3] = [
    ("Label_1", "Receipts"),
    ("Label_2", "Travel"),
    ("Label_3", "Projects"),
];

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

/// The thread that gets [`Size::long_thread`] messages.
const LONG_THREAD: usize = 3;
/// Minutes between one thread's latest message and the next older thread's.
const MINUTES_APART: i64 = 37;

/// Picks one of `items`.
fn pick<'a>(rng: &mut StdRng, items: &[&'a str]) -> &'a str {
    items
        .get(rng.random_range(0..items.len()))
        .copied()
        .unwrap_or_default()
}

/// Someone who writes to the account.
fn person(rng: &mut StdRng) -> Addr {
    let (first, last) = (pick(rng, &FIRST), pick(rng, &LAST));
    let domain = pick(rng, &DOMAINS);
    Addr::named(
        format!("{first} {last}"),
        format!("{}.{}@{domain}", first.to_lowercase(), last.to_lowercase()),
    )
}

/// A message body: HTML and its text.
fn body(rng: &mut StdRng) -> (String, String) {
    let lines: Vec<&str> = (0..rng.random_range(1..4))
        .map(|_| pick(rng, &LINES))
        .collect();
    let mut html = String::new();
    for line in &lines {
        html.push_str("<p>");
        html.push_str(line);
        html.push_str("</p>");
    }
    (html, lines.join(" "))
}

/// Writes the fixture into a new database at `path`: one account per
/// address in `accounts`, each of `size`, with message times counting back
/// from `now`. The same arguments write the same data.
pub fn generate(path: &Path, size: Size, accounts: &[&str], now: Millis) -> Result<(), StoreError> {
    let writer = Writer::start(path, (), |(), _, _| {}, |()| {})?;
    let mut rng = StdRng::seed_from_u64(7);
    for (n, email) in accounts.iter().enumerate() {
        let email = (*email).to_owned();
        let seed: u64 = rng.random();
        writer.call_blocking(Priority::Background, move |tx, ()| {
            let mut rng = StdRng::seed_from_u64(seed);
            write_account(tx, &mut rng, n, &email, size, now)
        })?;
    }
    writer.stop().wait()
}

/// One account and its mail, in one transaction.
fn write_account(
    tx: &mut WriteTx<'_>,
    rng: &mut StdRng,
    n: usize,
    email: &str,
    size: Size,
    now: Millis,
) -> Result<(), StoreError> {
    let name = email.split('@').next().unwrap_or(email).to_owned();
    let account = tx.accounts().create(email, &name)?;
    let me = Addr::named(name.clone(), email.to_owned());
    tx.accounts().set_profile(
        account,
        &name,
        &[SendAsAddress {
            email: email.to_owned(),
            name: name.clone(),
            is_default: true,
            is_primary: true,
        }],
    )?;
    tx.accounts().set_history(account, "1", now)?;
    tx.accounts().set_connected(account)?;
    let gmail_labels: Vec<GmailLabel> = LABELS
        .iter()
        .map(|(id, name)| GmailLabel {
            gmail_id: LabelId::from(*id),
            name: (*name).to_owned(),
            system: false,
        })
        .collect();
    tx.labels().sync_from_gmail(account, &gmail_labels)?;
    let mut user_labels = Vec::with_capacity(LABELS.len());
    for (id, _) in LABELS {
        if let Some(label) = tx
            .read()
            .labels()
            .by_gmail_id(account, &LabelId::from(id))?
        {
            user_labels.push(label);
        }
    }
    for t in 0..size.threads {
        let (fetched, bodies) = thread(rng, account, n, t, &me, size, now, &user_labels);
        let stored = tx.threads().store(&fetched)?;
        for (id, body) in stored.added.into_iter().zip(bodies) {
            tx.messages().store_body(id, &body)?;
        }
    }
    for c in 0..size.contacts {
        let who = person(rng);
        let sighting = Sighting {
            email: format!("{c}{}", who.email),
            name: who.name.unwrap_or_default(),
            sent: u32::from(rng.random_bool(0.5)),
            received: 1,
            on_list: false,
            at: now.plus(-rng.random_range(0..31_536_000_000_i64)),
        };
        let source = GmailMessageId::from(format!("c{n}_{c:05}"));
        tx.contacts().count(account, &source, &[sighting])?;
    }
    tracing::info!(
        account = account.0,
        threads = size.threads,
        "wrote a fixture account"
    );
    Ok(())
}

/// Thread `t` of account number `n`, as a full fetch would bring it, with
/// each message's body. Thread [`LONG_THREAD`] is the long one; the first
/// `size.inbox` are in the Inbox.
#[expect(
    clippy::too_many_arguments,
    reason = "one thread's inputs, from the account loop"
)]
fn thread(
    rng: &mut StdRng,
    account: AccountId,
    n: usize,
    t: usize,
    me: &Addr,
    size: Size,
    now: Millis,
    user_labels: &[LabelId],
) -> (FetchedThread, Vec<NewBody>) {
    let in_inbox = t < size.inbox;
    let count = if t == LONG_THREAD {
        size.long_thread
    } else {
        rng.random_range(1..5)
    };
    let subject = pick(rng, &SUBJECTS);
    let sender = person(rng);
    let age = i64::try_from(t)
        .unwrap_or(i64::MAX)
        .saturating_mul(MINUTES_APART * 60_000);
    let latest = now.plus(-age.saturating_add(rng.random_range(0..600_000)));
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
    let user_label = if rng.random_bool(0.05) {
        user_labels
            .get(rng.random_range(0..user_labels.len().max(1)))
            .cloned()
    } else {
        None
    };
    let mut messages = Vec::with_capacity(count);
    let mut bodies = Vec::with_capacity(count);
    for m in 0..count {
        let newest = m + 1 == count;
        let from_me = m % 2 == 1 && rng.random_bool(0.7);
        let (from, to) = if from_me {
            (me.clone(), sender.clone())
        } else {
            (sender.clone(), me.clone())
        };
        let mut labels: Vec<&str> = Vec::new();
        if from_me {
            labels.push("SENT");
        } else {
            if in_inbox {
                labels.push("INBOX");
            }
            labels.extend(category);
            if newest && in_inbox && rng.random_bool(0.35) {
                labels.push("UNREAD");
            }
        }
        if starred && newest {
            labels.push("STARRED");
        }
        labels.extend(user_label.as_ref().map(LabelId::as_str));
        let (html, text) = body(rng);
        let back = i64::try_from(count - 1 - m)
            .unwrap_or(0)
            .saturating_mul(3_600_000);
        messages.push(NewMessage {
            gmail_id: GmailMessageId::from(format!("m{n}_{t:06}_{m:02}")),
            date: latest.plus(-back),
            from,
            to: vec![to],
            cc: Vec::new(),
            bcc: Vec::new(),
            reply_to: Vec::new(),
            subject: if m == 0 {
                subject.to_owned()
            } else {
                format!("Re: {subject}")
            },
            snippet: text.chars().take(140).collect(),
            message_id_header: Some(format!("<m{n}.{t}.{m}@fixture>")),
            in_reply_to: None,
            references: String::new(),
            is_list,
            is_draft: false,
            ub_draft: None,
            labels: labels.into_iter().collect::<LabelSet>(),
            has_attachments: false,
        });
        bodies.push(NewBody {
            html: Some(html),
            quote_html: None,
            text: Some(text),
            is_plain: false,
            attachments: Vec::new(),
        });
    }
    let fetched = FetchedThread {
        account,
        gmail_id: GmailThreadId::from(format!("t{n}_{t:06}")),
        history_id: Some("1".to_owned()),
        messages,
        complete: true,
    };
    (fetched, bodies)
}
