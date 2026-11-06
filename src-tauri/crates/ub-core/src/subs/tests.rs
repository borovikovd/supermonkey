//! First loads and patches over snapshots the store builds: the registry
//! runs in a bare store writer's `on_commit`, as the core runs it.

use std::collections::BTreeSet;
use std::sync::Arc;

use parking_lot::Mutex;
use tempfile::TempDir;
use ub_domain::{
    AccountId, Addr, DraftId, DraftKind, GmailMessageId, GmailThreadId, HistoryId, LabelId,
    LabelSet, Millis, NoticeKind, Pin, PinKind, SplitId, SplitKind, SubId, SyncStatus, ThreadId,
    UndoEntryId, View,
};
use ub_ipc::{ListEntry, ListMove, SubKey, SubUpdate};
use ub_store::{
    Changes, Draft, FetchedThread, NewBody, NewMessage, NewNotice, NewOp, NewOutbox, OpKind,
    Priority, Reads, Sighting, SplitEdit, StoreError, Table, UndoAction, WriteTx, Writer,
};

use super::lists::FIRST_WINDOW;
use super::registry::{Registry, Updates};

struct Subs {
    _dir: TempDir,
    writer: Writer<Registry>,
    sent: Arc<Mutex<Vec<Updates>>>,
}

impl Subs {
    fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let sent: Arc<Mutex<Vec<Updates>>> = Arc::default();
        let out = Arc::clone(&sent);
        let writer = Writer::start(
            &dir.path().join("mail.db"),
            Registry::default(),
            move |registry: &mut Registry, snapshot, commit| {
                out.lock()
                    .push(registry.updates(**snapshot, &commit.changes, true, Millis(0)));
            },
            |_: &mut Registry| {},
        )
        .unwrap();
        Self {
            _dir: dir,
            writer,
            sent,
        }
    }

    /// Runs a job and returns the updates its commit made.
    fn job(
        &self,
        job: impl FnOnce(&mut WriteTx<'_>, &mut Registry) -> Result<(), StoreError> + Send + 'static,
    ) -> Updates {
        self.run(job).1
    }

    /// Runs a job and returns its result and the updates its commit made.
    fn run<T: Send + 'static>(
        &self,
        job: impl FnOnce(&mut WriteTx<'_>, &mut Registry) -> Result<T, StoreError> + Send + 'static,
    ) -> (T, Updates) {
        let value = self.writer.call_blocking(Priority::Ui, job).unwrap();
        (value, self.sent.lock().pop().unwrap())
    }

    fn open(&self, id: &str, key: SubKey) -> Updates {
        let id = SubId(id.to_owned());
        self.job(move |_, registry| {
            registry.open(id, key, false);
            Ok(())
        })
    }

    fn write(
        &self,
        job: impl FnOnce(&mut WriteTx<'_>) -> Result<(), StoreError> + Send + 'static,
    ) -> Updates {
        self.job(move |tx, _| job(tx))
    }
}

fn message(id: &str, date: i64, labels: &[&str]) -> NewMessage {
    NewMessage {
        gmail_id: GmailMessageId::from(id),
        date: Millis(date),
        from: Addr::named("Sender", "sender@example.org"),
        to: vec![Addr::new("a@example.com")],
        cc: Vec::new(),
        bcc: Vec::new(),
        reply_to: Vec::new(),
        subject: format!("Subject {id}"),
        snippet: format!("Snippet {id}"),
        message_id_header: None,
        in_reply_to: None,
        references: String::new(),
        is_list: false,
        is_draft: false,
        ub_draft: None,
        labels: labels.iter().copied().collect::<LabelSet>(),
        has_attachments: false,
    }
}

/// An account with `count` Inbox threads, newest first by index.
fn mailbox(tx: &mut WriteTx<'_>, count: i64) -> Result<(AccountId, Vec<ThreadId>), StoreError> {
    let account = tx.accounts().create("a@example.com", "A")?;
    let mut threads = Vec::new();
    for t in 0..count {
        let stored = tx.threads().store(&FetchedThread {
            account,
            gmail_id: GmailThreadId::from(format!("t{t}")),
            history_id: None,
            messages: vec![message(&format!("m{t}"), 1_000_000 - t, &["INBOX"])],
            complete: true,
        })?;
        threads.push(stored.thread.unwrap());
    }
    Ok((account, threads))
}

/// Seeds a mailbox and returns its account and threads.
fn seeded(subs: &Subs, count: i64) -> (AccountId, Vec<ThreadId>) {
    subs.writer
        .call_blocking(Priority::Ui, move |tx, _: &mut Registry| mailbox(tx, count))
        .unwrap()
}

fn inbox(account: AccountId) -> SubKey {
    SubKey::List {
        account,
        view: View::Inbox,
    }
}

struct ListParts {
    first: bool,
    ids: Option<Vec<ListEntry>>,
    removed: Vec<String>,
    moved: Vec<ListMove>,
    rows: Vec<String>,
}

fn list(update: &SubUpdate) -> ListParts {
    let SubUpdate::List {
        first,
        ids,
        removed,
        moved,
        rows,
        ..
    } = update
    else {
        panic!("not a list update: {update:?}");
    };
    ListParts {
        first: *first,
        ids: ids.clone(),
        removed: removed.clone(),
        moved: moved.clone(),
        rows: rows.iter().map(|r| r.key.clone()).collect(),
    }
}

fn sub_of(update: &SubUpdate) -> &str {
    match update {
        SubUpdate::List { sub, .. }
        | SubUpdate::Conversation { sub, .. }
        | SubUpdate::Composer { sub, .. }
        | SubUpdate::Account { sub, .. }
        | SubUpdate::App { sub, .. } => sub.as_str(),
    }
}

#[test]
fn a_first_load_holds_every_id_and_the_rows_of_the_top_window() {
    let subs = Subs::start();
    let (account, threads) = seeded(&subs, 100);
    let updates = subs.open("s1", inbox(account));
    assert_eq!(updates.patches, []);
    let [first] = updates.first_loads.as_slice() else {
        panic!("one first load: {:?}", updates.first_loads);
    };
    let first = list(first);
    assert!(first.first);
    let ids: Vec<String> = first.ids.unwrap().into_iter().map(|e| e.key).collect();
    let expected: Vec<String> = threads.iter().map(ToString::to_string).collect();
    assert_eq!(ids, expected, "newest first");
    assert_eq!(first.rows, expected[..FIRST_WINDOW], "the top 80 rows only");
}

#[test]
fn first_loads_go_out_in_subscribe_order_after_the_patches() {
    let subs = Subs::start();
    let (account, threads) = seeded(&subs, 3);
    subs.open("s9", inbox(account));
    let key = |id: &str, key: SubKey| (SubId(id.to_owned()), key);
    let opened = [
        key("s5", SubKey::App),
        key("s2", SubKey::Account { account }),
        key(
            "s7",
            SubKey::Conversation {
                thread: threads[0],
                trash_mode: false,
                open: true,
            },
        ),
    ];
    let thread = threads[2];
    let updates = subs.job(move |tx, registry| {
        for (id, key) in opened {
            registry.open(id, key, false);
        }
        // The same commit moves the open Inbox: its patch comes first.
        tx.messages()
            .upsert(thread, &message("m2", 2_000_000, &["INBOX"]))?;
        Ok(())
    });
    let patched: Vec<&str> = updates.patches.iter().map(sub_of).collect();
    assert_eq!(patched, ["s9"]);
    let order: Vec<&str> = updates.first_loads.iter().map(sub_of).collect();
    assert_eq!(order, ["s5", "s2", "s7"]);
}

#[test]
fn a_patch_has_moves_removals_and_the_changed_rows_outside_the_window() {
    let subs = Subs::start();
    let (account, threads) = seeded(&subs, 100);
    subs.open("s1", inbox(account));
    let (archived, bumped) = (threads[90], threads[85]);
    let updates = subs.write(move |tx| {
        let m = tx.read().messages().for_thread(archived)?.remove(0);
        tx.messages().set_labels(&[(m.id, LabelSet::new())])?;
        tx.messages()
            .upsert(bumped, &message("m85", 2_000_000, &["INBOX"]))?;
        Ok(())
    });
    assert_eq!(updates.first_loads, []);
    let [patch] = updates.patches.as_slice() else {
        panic!("one patch: {:?}", updates.patches);
    };
    let patch = list(patch);
    assert!(!patch.first);
    assert_eq!(patch.ids, None, "patches carry no ids");
    assert_eq!(patch.removed, [archived.to_string()]);
    assert_eq!(
        patch.moved,
        [ListMove {
            key: bumped.to_string(),
            split: patch.moved[0].split,
            after: None,
        }],
        "the bumped thread goes to the top"
    );
    assert_eq!(
        patch.rows,
        [bumped.to_string()],
        "a changed row outside the window goes along; a removed one does not"
    );
}

#[test]
fn a_commit_that_changes_nothing_shown_sends_no_patch() {
    let subs = Subs::start();
    let (account, threads) = seeded(&subs, 3);
    subs.open("s1", inbox(account));
    subs.open("s2", SubKey::Account { account });
    let thread = threads[0];
    // Another account's mail, and a history id nothing shows.
    let updates = subs.write(move |tx| {
        let other = tx.accounts().create("b@example.com", "B")?;
        tx.threads().set_history_id(thread, HistoryId(99))?;
        tx.threads().store(&FetchedThread {
            account: other,
            gmail_id: GmailThreadId::from("x"),
            history_id: None,
            messages: vec![message("x1", 5, &["INBOX"])],
            complete: true,
        })?;
        Ok(())
    });
    assert_eq!(updates.patches, [], "{:?}", updates.patches);
}

#[test]
fn a_change_in_place_sends_only_its_row() {
    let subs = Subs::start();
    let (account, threads) = seeded(&subs, 3);
    subs.open("s1", inbox(account));
    let thread = threads[1];
    let updates = subs.write(move |tx| {
        let m = tx.read().messages().for_thread(thread)?.remove(0);
        let labels = ["INBOX", "STARRED"].into_iter().collect::<LabelSet>();
        tx.messages().set_labels(&[(m.id, labels)])?;
        Ok(())
    });
    let patch = list(&updates.patches[0]);
    assert_eq!((patch.removed, patch.moved), (vec![], vec![]));
    assert_eq!(patch.rows, [thread.to_string()]);
}

#[test]
fn a_forced_app_patch_goes_out_with_nothing_changed() {
    let subs = Subs::start();
    subs.open("app", SubKey::App);
    let quiet = subs.job(|_, _| Ok(()));
    assert_eq!(quiet.patches, []);
    let forced = subs.job(|_, registry| {
        registry.force_app();
        Ok(())
    });
    assert!(
        matches!(
            forced.patches.as_slice(),
            [SubUpdate::App { first: false, .. }]
        ),
        "{:?}",
        forced.patches
    );
    let after = subs.job(|_, _| Ok(()));
    assert_eq!(after.patches, [], "forced once");
}

#[test]
fn an_account_summary_starts_empty_and_patches_once_the_account_exists() {
    let subs = Subs::start();
    let first = subs.open(
        "s1",
        SubKey::Account {
            account: AccountId(1),
        },
    );
    assert!(matches!(
        first.first_loads.as_slice(),
        [SubUpdate::Account { first: true, summary, .. }] if summary.account.email.is_empty()
    ));
    let created = subs.write(|tx| tx.accounts().create("a@example.com", "A").map(drop));
    assert!(matches!(
        created.patches.as_slice(),
        [SubUpdate::Account { first: false, summary, .. }]
            if summary.account.email == "a@example.com" && !summary.splits.is_empty()
    ));
}

#[test]
fn the_drafts_list_is_keyed_by_draft_id() {
    let subs = Subs::start();
    let account = subs
        .writer
        .call_blocking(Priority::Ui, |tx, _: &mut Registry| {
            tx.accounts().create("a@example.com", "A")
        })
        .unwrap();
    subs.open(
        "s1",
        SubKey::List {
            account,
            view: View::Drafts,
        },
    );
    let updates = subs.write(move |tx| {
        let draft = Draft::new(DraftId::from("d1"), account, DraftKind::New, Millis(5));
        tx.drafts().put(&draft)
    });
    let patch = list(&updates.patches[0]);
    assert_eq!(
        patch.moved,
        [ListMove {
            key: "d1".to_owned(),
            split: None,
            after: None,
        }]
    );
    assert_eq!(patch.rows, ["d1"]);
}

#[test]
fn a_conversation_for_a_thread_that_is_gone_says_so() {
    let subs = Subs::start();
    let updates = subs.open(
        "s1",
        SubKey::Conversation {
            thread: ThreadId(404),
            trash_mode: false,
            open: true,
        },
    );
    assert!(matches!(
        updates.first_loads.as_slice(),
        [SubUpdate::Conversation { first: true, data, .. }] if data.gone
    ));
}

/// A write to the store, named for the failure message.
type Write = (
    &'static str,
    Box<dyn FnOnce(&mut WriteTx<'_>) -> Result<(), StoreError> + Send>,
);

/// What each subscription shows now, as a fresh first load builds it.
fn shown(reads: Reads<'_>, keys: &[(SubId, SubKey)]) -> Vec<SubUpdate> {
    let mut fresh = Registry::default();
    for (id, key) in keys {
        fresh.open(id.clone(), key.clone(), false);
    }
    fresh
        .updates(reads, &Changes::default(), true, Millis(0))
        .first_loads
}

/// One write of each table an account owns, as the app makes them.
#[expect(
    clippy::too_many_lines,
    reason = "one short write per table, listed together so none is missed"
)]
fn one_write_per_table(account: AccountId, threads: &[ThreadId], draft: &DraftId) -> Vec<Write> {
    let (first, second, third) = (threads[0], threads[1], threads[2]);
    let (sent, outboxed) = (draft.clone(), draft.clone());
    vec![
        (
            "a sync state",
            Box::new(move |tx| {
                tx.accounts()
                    .set_sync_status(account, SyncStatus::Syncing)
                    .map(drop)
            }),
        ),
        (
            "a history cursor",
            Box::new(move |tx| {
                tx.accounts()
                    .set_history(account, HistoryId(5), Millis(1))?;
                tx.accounts().set_connected(account)?;
                tx.threads().set_history_id(second, HistoryId(6))
            }),
        ),
        (
            "a label",
            Box::new(move |tx| {
                tx.labels()
                    .create_local(account, &LabelId::from("local-1"), "Work")
            }),
        ),
        (
            "a message's labels",
            Box::new(move |tx| {
                let m = tx.read().messages().for_thread(second)?.remove(0);
                let labels = ["INBOX", "UNREAD"].into_iter().collect::<LabelSet>();
                tx.messages().set_labels(&[(m.id, labels)])
            }),
        ),
        (
            "a body",
            Box::new(move |tx| {
                let m = tx.read().messages().for_thread(first)?.remove(0);
                tx.messages().store_body(
                    m.id,
                    &NewBody {
                        html: Some("<p>Hi</p>".to_owned()),
                        quote_html: None,
                        text: Some("Hi".to_owned()),
                        is_plain: false,
                        attachments: Vec::new(),
                    },
                )
            }),
        ),
        (
            "a snooze",
            Box::new(move |tx| tx.threads().set_snooze(third, Some(Millis(10_000)))),
        ),
        (
            "a draft",
            Box::new(move |tx| {
                let mut d = tx.read().drafts().get(&sent)?.unwrap().draft;
                d.subject = "Edited".to_owned();
                tx.drafts().put(&d)
            }),
        ),
        (
            "a send",
            Box::new(move |tx| {
                tx.outbox()
                    .insert(&NewOutbox {
                        account,
                        draft: outboxed,
                        thread: Some(first),
                        send_after: Millis(100),
                    })
                    .map(drop)
            }),
        ),
        (
            "an op",
            Box::new(move |tx| {
                tx.ops()
                    .push(&NewOp {
                        account,
                        action: None,
                        kind: OpKind::LabelCreate,
                        draft: None,
                        gmail_draft: None,
                        payload: &"Work",
                        created_at: Millis(1),
                    })
                    .map(drop)
            }),
        ),
        (
            "an undo entry",
            Box::new(move |tx| {
                tx.undo().record(
                    account,
                    UndoEntryId(1),
                    "Archive",
                    &UndoAction::Mail {
                        threads: Vec::new(),
                    },
                )
            }),
        ),
        (
            "a notice",
            Box::new(move |tx| {
                tx.notices()
                    .add(&NewNotice {
                        account,
                        kind: NoticeKind::Info,
                        message: "Hello".to_owned(),
                        draft: None,
                        thread: None,
                        created_at: Millis(1),
                    })
                    .map(drop)
            }),
        ),
        (
            "a split",
            Box::new(move |tx| {
                tx.splits()
                    .save(
                        account,
                        &[SplitEdit {
                            id: SplitId(0),
                            name: "Sender".to_owned(),
                            kind: SplitKind::Custom,
                            query: "from:sender@example.org".to_owned(),
                            enabled: true,
                            hide_when_empty: false,
                        }],
                        &[],
                    )
                    .map(drop)
            }),
        ),
        (
            "a pin",
            Box::new(move |tx| {
                let other = tx
                    .read()
                    .splits()
                    .all(account)?
                    .into_iter()
                    .find(|s| s.kind == SplitKind::Other)
                    .unwrap();
                let pin = Pin::new(PinKind::Sender, "sender@example.org", other.id).unwrap();
                tx.splits().set_pin(account, &pin).map(drop)
            }),
        ),
        (
            "a mute",
            Box::new(move |tx| tx.settings().set(account, "muted", "on")),
        ),
        (
            "a global setting",
            Box::new(|tx| tx.settings().set(AccountId::NONE, "remote_images", "off")),
        ),
        (
            "a contact",
            Box::new(move |tx| {
                tx.contacts()
                    .count(
                        account,
                        &GmailMessageId::from("m0"),
                        &[Sighting {
                            email: "sender@example.org".to_owned(),
                            name: "Sender".to_owned(),
                            sent: 0,
                            received: 1,
                            on_list: false,
                            at: Millis(1),
                        }],
                    )
                    .map(drop)
            }),
        ),
    ]
}

/// The mapping from what the store wrote to the read models it makes stale
/// (`affects`), held to what the views actually read: a write to each table
/// changes the subscriptions that show it, and each one that changed gets a
/// patch with the commit.
#[test]
fn every_write_that_changes_what_a_subscription_shows_patches_it() {
    let subs = Subs::start();
    let (account, threads) = seeded(&subs, 3);
    let draft = DraftId::from("d1");
    let (seed, on) = (draft.clone(), threads[0]);
    subs.write(move |tx| {
        let mut d = Draft::new(seed, account, DraftKind::Reply, Millis(5));
        d.thread = Some(on);
        d.to = vec![Addr::new("sender@example.org")];
        tx.drafts().put(&d)
    });
    let id = |name: &str| SubId(name.to_owned());
    let keys = vec![
        (id("app"), SubKey::App),
        (id("summary"), SubKey::Account { account }),
        (id("inbox"), inbox(account)),
        (
            id("drafts"),
            SubKey::List {
                account,
                view: View::Drafts,
            },
        ),
        (
            id("search"),
            SubKey::Search {
                account,
                query: "subject".to_owned(),
            },
        ),
        (
            id("conversation"),
            SubKey::Conversation {
                thread: threads[0],
                trash_mode: false,
                open: true,
            },
        ),
        (
            id("composer"),
            SubKey::Composer {
                draft: draft.clone(),
            },
        ),
    ];
    for (id, key) in &keys {
        subs.open(&id.0, key.clone());
    }
    let mut touched = BTreeSet::new();
    for (name, write) in one_write_per_table(account, &threads, &draft) {
        let watched = keys.clone();
        let ((changed, tables), updates) = subs.run(move |tx, _| {
            let before = shown(tx.read(), &watched);
            write(tx)?;
            let after = shown(tx.read(), &watched);
            let changed: Vec<String> = before
                .iter()
                .zip(&after)
                .filter(|(b, a)| b != a)
                .map(|(_, a)| sub_of(a).to_owned())
                .collect();
            Ok((changed, tx.changes().tables.clone()))
        });
        touched.extend(tables.into_iter().map(|(_, table)| table));
        let patched: Vec<&str> = updates.patches.iter().map(sub_of).collect();
        for sub in &changed {
            assert!(
                patched.contains(&sub.as_str()),
                "{name} changed {sub} but did not patch it (patched {patched:?})"
            );
        }
    }
    let missing: Vec<Table> = Table::ALL
        .into_iter()
        .filter(|t| !touched.contains(t))
        .collect();
    assert_eq!(missing, [], "a table with no write here");
}
