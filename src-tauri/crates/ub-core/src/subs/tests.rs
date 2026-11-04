//! First loads and patches over snapshots the store builds: the registry
//! runs in a bare store writer's `on_commit`, as the core runs it.

use std::sync::Arc;

use parking_lot::Mutex;
use tempfile::TempDir;
use ub_domain::{
    AccountId, Addr, DraftId, DraftKind, GmailMessageId, GmailThreadId, LabelSet, Millis, SubId,
    ThreadId, View,
};
use ub_ipc::{ListEntry, ListMove, SubKey, SubUpdate};
use ub_store::{Draft, FetchedThread, NewMessage, Priority, StoreError, WriteTx, Writer};

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
                    .push(registry.updates(**snapshot, &commit.changes, true));
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
        self.writer.call_blocking(Priority::Ui, job).unwrap();
        self.sent.lock().pop().unwrap()
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
        tx.threads().set_history_id(thread, "99")?;
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
