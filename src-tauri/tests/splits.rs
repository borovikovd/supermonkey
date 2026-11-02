//! Split routing (SPEC Split Inbox → Where a conversation lands).

use rusqlite::params;
use ubermensch_lib::model::*;
use ubermensch_lib::writer::Writer;

struct T {
    writer: Writer,
    path: std::path::PathBuf,
    _dir: tempfile::TempDir,
    n: i64,
}

impl T {
    fn new() -> T {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let (etx, _erx) = tokio::sync::mpsc::unbounded_channel();
        let writer = Writer::start(path.clone(), etx).unwrap();
        writer
            .call_blocking(|ctx| {
                let a = ubermensch_lib::accounts::create(ctx, "me@example.com")?;
                ctx.tx.execute(
                    "INSERT INTO labels (account_id, id, gmail_id, name, system) VALUES (?1, 'L1', 'L1', 'Clients', 0)",
                    [a],
                )?;
                Ok(())
            })
            .unwrap();
        T {
            writer,
            path,
            _dir: dir,
            n: 0,
        }
    }

    /// Adds an inbox thread with one message.
    fn thread(&self, from: &str, subject: &str, labels: &[&str], list: bool) -> ThreadId {
        let (from, subject) = (from.to_string(), subject.to_string());
        let labels: Vec<String> = labels.iter().map(|s| s.to_string()).collect();
        self.writer
            .call_blocking(move |ctx| {
                let gid = format!("t{}", ctx.now + rand_id());
                ctx.tx
                    .execute("INSERT INTO threads (account_id, gmail_id) VALUES (1, ?1)", [&gid])?;
                let t = ctx.tx.last_insert_rowid();
                let mut all = vec!["INBOX".to_string()];
                all.extend(labels);
                ctx.tx.execute(
                    "INSERT INTO messages (account_id, thread_id, gmail_id, internal_date, from_email, subject, label_ids, is_list)
                     VALUES (1, ?1, ?2, 1, ?3, ?4, ?5, ?6)",
                    params![t, format!("m{gid}"), from, subject, serde_json::to_string(&all)?, list],
                )?;
                ubermensch_lib::threads::refresh(ctx, t)?;
                Ok(t)
            })
            .unwrap()
    }

    fn split_kind(&self, t: ThreadId) -> String {
        let c = rusqlite::Connection::open(&self.path).unwrap();
        c.query_row(
            "SELECT coalesce(s.kind || ':' || s.name, 'none') FROM threads t LEFT JOIN splits s ON s.id = t.split_id WHERE t.id = ?1",
            [t],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn splits(&self) -> Vec<SplitInfo> {
        let c = rusqlite::Connection::open(&self.path).unwrap();
        ubermensch_lib::db::queries::splits(&c, 1).unwrap()
    }

    fn act(&mut self, a: Action) {
        self.n += 1;
        self.writer.actions(vec![NumberedAction {
            n: self.n,
            action: a,
        }]);
        self.writer.call_blocking(|_| Ok(())).unwrap();
    }
}

fn rand_id() -> i64 {
    use std::sync::atomic::{AtomicI64, Ordering};
    static N: AtomicI64 = AtomicI64::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

#[test]
fn important_is_person_to_person_and_other_catches_the_rest() {
    let t = T::new();
    let person = t.thread("ada@acme.io", "Hi", &[], false);
    let personal = t.thread("ada@acme.io", "Hi", &["CATEGORY_PERSONAL"], false);
    let promo = t.thread("deals@shop.com", "Sale", &["CATEGORY_PROMOTIONS"], false);
    let list = t.thread("news@list.org", "Digest", &[], true);
    assert_eq!(t.split_kind(person), "important:Important");
    assert_eq!(t.split_kind(personal), "important:Important");
    assert_eq!(t.split_kind(promo), "other:Other");
    assert_eq!(t.split_kind(list), "other:Other");
}

#[test]
fn custom_splits_come_first_and_pins_beat_rules() {
    let mut t = T::new();
    let mut splits = t.splits();
    splits.insert(
        0,
        SplitInfo {
            id: 0,
            name: "Newsletters".into(),
            kind: SplitKind::Custom,
            query: "is:list".into(),
            enabled: true,
            hide_when_empty: true,
            position: 0,
        },
    );
    t.act(Action::SaveSplits {
        account: 1,
        splits,
        deleted: vec![],
    });
    let news = t.thread("news@list.org", "Digest", &[], true);
    let ada = t.thread("ada@acme.io", "Hi", &["L1"], false);
    assert_eq!(t.split_kind(news), "custom:Newsletters");
    assert_eq!(t.split_kind(ada), "important:Important");

    // A label pin moves Ada's thread; a sender pin beats it.
    let newsletters = t
        .splits()
        .into_iter()
        .find(|s| s.name == "Newsletters")
        .unwrap()
        .id;
    let other = t
        .splits()
        .into_iter()
        .find(|s| s.kind == SplitKind::Other)
        .unwrap()
        .id;
    let important = t
        .splits()
        .into_iter()
        .find(|s| s.kind == SplitKind::Important)
        .unwrap()
        .id;
    t.act(Action::Pin {
        account: 1,
        kind: PinKind::Label,
        value: "Clients".into(),
        split: other,
    });
    assert_eq!(t.split_kind(ada), "other:Other");
    t.act(Action::Pin {
        account: 1,
        kind: PinKind::Sender,
        value: "ADA@acme.io".into(),
        split: newsletters,
    });
    assert_eq!(t.split_kind(ada), "custom:Newsletters");
    // A pin to a split that is turned off is skipped.
    let mut splits = t.splits();
    for s in splits.iter_mut() {
        if s.id == newsletters {
            s.enabled = false;
        }
    }
    t.act(Action::SaveSplits {
        account: 1,
        splits,
        deleted: vec![],
    });
    assert_eq!(t.split_kind(ada), "other:Other");
    assert_eq!(t.split_kind(news), "other:Other");
    // Undo restores the pin it replaced (none), so the label pin wins again.
    let _ = important;
}

#[test]
fn undoing_a_pin_restores_the_one_it_replaced() {
    let mut t = T::new();
    let ada = t.thread("ada@acme.io", "Hi", &[], false);
    let other = t
        .splits()
        .into_iter()
        .find(|s| s.kind == SplitKind::Other)
        .unwrap()
        .id;
    let important = t
        .splits()
        .into_iter()
        .find(|s| s.kind == SplitKind::Important)
        .unwrap()
        .id;
    t.act(Action::Pin {
        account: 1,
        kind: PinKind::Domain,
        value: "acme.io".into(),
        split: other,
    });
    assert_eq!(t.split_kind(ada), "other:Other");
    let first = t.n;
    t.act(Action::Pin {
        account: 1,
        kind: PinKind::Domain,
        value: "acme.io".into(),
        split: important,
    });
    assert_eq!(t.split_kind(ada), "important:Important");
    let second = t.n;
    t.act(Action::Undo {
        account: 1,
        entry: second,
    });
    assert_eq!(t.split_kind(ada), "other:Other");
    t.act(Action::Undo {
        account: 1,
        entry: first,
    });
    assert_eq!(t.split_kind(ada), "important:Important");
}

#[test]
fn deleting_a_custom_split_lets_its_conversations_fall_through() {
    let mut t = T::new();
    let mut splits = t.splits();
    splits.insert(
        0,
        SplitInfo {
            id: 0,
            name: "Acme".into(),
            kind: SplitKind::Custom,
            query: "from:acme.io".into(),
            enabled: true,
            hide_when_empty: true,
            position: 0,
        },
    );
    t.act(Action::SaveSplits {
        account: 1,
        splits,
        deleted: vec![],
    });
    let ada = t.thread("ada@acme.io", "Hi", &[], false);
    assert_eq!(t.split_kind(ada), "custom:Acme");
    let acme = t
        .splits()
        .into_iter()
        .find(|s| s.name == "Acme")
        .unwrap()
        .id;
    let rest: Vec<SplitInfo> = t.splits().into_iter().filter(|s| s.id != acme).collect();
    t.act(Action::SaveSplits {
        account: 1,
        splits: rest,
        deleted: vec![acme],
    });
    assert_eq!(t.split_kind(ada), "important:Important");
}

#[test]
fn own_reply_does_not_move_a_thread() {
    let t = T::new();
    let th = t.thread("deals@shop.com", "Sale", &["CATEGORY_PROMOTIONS"], false);
    assert_eq!(t.split_kind(th), "other:Other");
    t.writer
        .call_blocking(move |ctx| {
            ctx.tx.execute(
                "INSERT INTO messages (account_id, thread_id, gmail_id, internal_date, from_email, subject, label_ids)
                 VALUES (1, ?1, 'mine', 99, 'me@example.com', 'Re: Sale', '[\"SENT\",\"INBOX\"]')",
                [th],
            )?;
            ubermensch_lib::threads::refresh(ctx, th)?;
            Ok(())
        })
        .unwrap();
    assert_eq!(t.split_kind(th), "other:Other");
}
