use std::path::Path;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use sm_domain::{
    AccountId, Addr, GmailMessageId, GmailThreadId, LabelId, LabelSet, Millis, SendAsAddress,
};
use sm_store::{
    FetchedThread, GmailLabel, NewBody, NewMessage, Priority, Sighting, StoreError, WriteTx, Writer,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

const LONG_THREAD: usize = 3;
const MINUTES_APART: i64 = 37;

fn pick<'a>(rng: &mut StdRng, items: &[&'a str]) -> &'a str {
    items
        .get(rng.random_range(0..items.len()))
        .copied()
        .unwrap_or_default()
}

fn person(rng: &mut StdRng) -> Addr {
    let (first, last) = (pick(rng, &FIRST), pick(rng, &LAST));
    let domain = pick(rng, &DOMAINS);
    Addr::named(
        format!("{first} {last}"),
        format!("{}.{}@{domain}", first.to_lowercase(), last.to_lowercase()),
    )
}

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

pub fn generate(path: &Path, size: Size, accounts: &[&str], now: Millis) -> Result<(), StoreError> {
    let writer = Writer::start(path, (), |(), _, _| {}, |()| {})?;
    let mut rng = StdRng::seed_from_u64(7);
    for (n, email) in accounts.iter().enumerate() {
        let email = (*email).to_owned();
        let seed: u64 = rng.random();
        let plan = Plan { n, size, now };
        writer.call_blocking(Priority::Background, move |tx, ()| {
            let mut rng = StdRng::seed_from_u64(seed);
            write_account(tx, &mut rng, plan, email)
        })?;
    }
    writer.stop().wait()
}

#[derive(Debug, Clone, Copy)]
struct Plan {
    n: usize,
    size: Size,
    now: Millis,
}

struct Owner {
    account: AccountId,
    me: Addr,
    labels: Vec<LabelId>,
}

fn write_account(
    tx: &mut WriteTx<'_>,
    rng: &mut StdRng,
    plan: Plan,
    email: String,
) -> Result<(), StoreError> {
    let owner = create_owner(tx, email)?;
    write_threads(tx, rng, plan, &owner)?;
    write_contacts(tx, rng, plan, owner.account)?;
    tracing::info!(
        account = owner.account.0,
        threads = plan.size.threads,
        "wrote a fixture account"
    );
    Ok(())
}

fn create_owner(tx: &mut WriteTx<'_>, email: String) -> Result<Owner, StoreError> {
    let name = email.split('@').next().unwrap_or(&email).to_owned();
    let account = tx.accounts().create(&email, &name)?;
    tx.accounts().set_profile(
        account,
        &name,
        &[SendAsAddress {
            email: email.clone(),
            name: name.clone(),
            is_default: true,
            is_primary: true,
        }],
    )?;
    let me = Addr::named(name, email);
    tx.accounts()
        .set_history(account, sm_domain::HistoryId(1))?;
    tx.accounts().set_connected(account)?;
    let labels = sync_labels(tx, account)?;
    Ok(Owner {
        account,
        me,
        labels,
    })
}

fn sync_labels(tx: &mut WriteTx<'_>, account: AccountId) -> Result<Vec<LabelId>, StoreError> {
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
    Ok(user_labels)
}

fn write_threads(
    tx: &mut WriteTx<'_>,
    rng: &mut StdRng,
    plan: Plan,
    owner: &Owner,
) -> Result<(), StoreError> {
    for t in 0..plan.size.threads {
        let (fetched, bodies) = thread(rng, plan, owner, t);
        let stored = tx.threads().store(&fetched)?;
        for (id, body) in stored.added.into_iter().zip(bodies) {
            tx.messages().store_body(id, &body)?;
        }
    }
    Ok(())
}

fn write_contacts(
    tx: &mut WriteTx<'_>,
    rng: &mut StdRng,
    plan: Plan,
    account: AccountId,
) -> Result<(), StoreError> {
    let Plan { n, size, now } = plan;
    for c in 0..size.contacts {
        let who = person(rng);
        let email = format!("{c}{}", who.email);
        let sighting = Sighting {
            email: &email,
            name: who.name.as_deref().unwrap_or_default(),
            sent: u32::from(rng.random_bool(0.5)),
            received: 1,
            on_list: false,
            at: now.plus(-rng.random_range(0..31_536_000_000_i64)),
        };
        let source = GmailMessageId::from(format!("c{n}_{c:05}"));
        tx.contacts().count(account, &source, &[sighting])?;
    }
    Ok(())
}

struct Traits<'a> {
    in_inbox: bool,
    category: Option<&'static str>,
    is_list: bool,
    starred: bool,
    user_label: Option<&'a LabelId>,
}

impl<'a> Traits<'a> {
    fn draw(rng: &mut StdRng, in_inbox: bool, user_labels: &'a [LabelId]) -> Self {
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
            user_labels.get(rng.random_range(0..user_labels.len().max(1)))
        } else {
            None
        };
        Self {
            in_inbox,
            category,
            is_list,
            starred,
            user_label,
        }
    }

    fn labels(&self, rng: &mut StdRng, from_me: bool, newest: bool) -> LabelSet {
        let mut labels: Vec<&str> = Vec::new();
        if from_me {
            labels.push("SENT");
        } else {
            if self.in_inbox {
                labels.push("INBOX");
            }
            labels.extend(self.category);
            if newest && self.in_inbox && rng.random_bool(0.35) {
                labels.push("UNREAD");
            }
        }
        if self.starred && newest {
            labels.push("STARRED");
        }
        labels.extend(self.user_label.map(LabelId::as_str));
        labels.into_iter().collect()
    }
}

fn thread(rng: &mut StdRng, plan: Plan, owner: &Owner, t: usize) -> (FetchedThread, Vec<NewBody>) {
    let Plan { n, size, now } = plan;
    let me = &owner.me;
    let in_inbox = t < size.inbox;
    let count = message_count(rng, size, t);
    let subject = pick(rng, &SUBJECTS);
    let sender = person(rng);
    let latest = latest_time(rng, now, t);
    let traits = Traits::draw(rng, in_inbox, &owner.labels);
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
        let labels = traits.labels(rng, from_me, newest);
        let (html, text) = body(rng);
        messages.push(NewMessage {
            gmail_id: GmailMessageId::from(format!("m{n}_{t:06}_{m:02}")),
            date: received_at(latest, count, m),
            from,
            to: vec![to],
            cc: Vec::new(),
            bcc: Vec::new(),
            reply_to: Vec::new(),
            subject: reply_subject(subject, m),
            snippet: text.chars().take(140).collect(),
            message_id_header: Some(format!("<m{n}.{t}.{m}@fixture>")),
            in_reply_to: None,
            references: String::new(),
            is_list: traits.is_list,
            is_draft: false,
            sm_draft: None,
            labels,
            has_attachments: false,
        });
        bodies.push(html_body(html, text));
    }
    let fetched = FetchedThread {
        account: owner.account,
        gmail_id: GmailThreadId::from(format!("t{n}_{t:06}")),
        history_id: Some(sm_domain::HistoryId(1)),
        messages,
        complete: true,
    };
    (fetched, bodies)
}

fn message_count(rng: &mut StdRng, size: Size, t: usize) -> usize {
    if t == LONG_THREAD {
        size.long_thread
    } else {
        rng.random_range(1..5)
    }
}

fn latest_time(rng: &mut StdRng, now: Millis, t: usize) -> Millis {
    let age = i64::try_from(t)
        .unwrap_or(i64::MAX)
        .saturating_mul(MINUTES_APART * 60_000);
    now.plus(-age.saturating_add(rng.random_range(0..600_000)))
}

fn received_at(latest: Millis, count: usize, m: usize) -> Millis {
    let back = i64::try_from(count - 1 - m)
        .unwrap_or(0)
        .saturating_mul(3_600_000);
    latest.plus(-back)
}

fn reply_subject(subject: &str, m: usize) -> String {
    if m == 0 {
        subject.to_owned()
    } else {
        format!("Re: {subject}")
    }
}

const fn html_body(html: String, text: String) -> NewBody {
    NewBody {
        html: Some(html),
        quote_html: None,
        text: Some(text),
        attachments: Vec::new(),
    }
}
