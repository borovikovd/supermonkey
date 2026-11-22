use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Arc;

use parking_lot::Mutex;
use proptest::prelude::*;
use sm_domain::{
    AccountId, ActionNo, LabelDelta, LabelId, LabelSet, Millis, ModifyKind, SnoozeEdit, SubId,
    ThreadId, UndoEntryId, View,
};
use sm_ipc::{
    Action, ChannelMessage, ListEntry, ListMove, ModifyAction, NumberedAction, SnoozeChange,
    SubKey, SubUpdate,
};
use sm_store::{NewMessage, Pending, Priority, ReadPool, StoreError};

use super::{Harness, NEWEST, eventually, seed};
use crate::App;
use crate::workers::Freshness;
use crate::writer::CoreWriter;

#[derive(Debug, Clone)]
enum Op {
    Subscribe(usize, View),
    Unsubscribe(usize),
    Modify(ModifyKind, Vec<usize>),
    Undo,
    Remote(usize, &'static str, bool),
    Bump(usize),
}

fn view() -> impl Strategy<Value = View> {
    prop_oneof![
        Just(View::Inbox),
        Just(View::Done),
        Just(View::Starred),
        Just(View::Trash),
        Just(View::Spam),
        Just(View::Snoozed),
        Just(View::Sent),
    ]
}

fn op() -> impl Strategy<Value = Op> {
    let kind = prop_oneof![
        Just(ModifyKind::Done),
        Just(ModifyKind::ReturnToInbox),
        Just(ModifyKind::Trash),
        Just(ModifyKind::Spam),
        Just(ModifyKind::Star),
        Just(ModifyKind::Unstar),
        Just(ModifyKind::Read),
        Just(ModifyKind::Unread),
        Just(ModifyKind::Snooze),
    ];
    prop_oneof![
        (0usize..4, view()).prop_map(|(s, v)| Op::Subscribe(s, v)),
        (0usize..4).prop_map(Op::Unsubscribe),
        (kind, proptest::collection::vec(0usize..40, 1..4)).prop_map(|(k, t)| Op::Modify(k, t)),
        Just(Op::Undo),
        (
            0usize..40,
            prop_oneof![Just("INBOX"), Just("CATEGORY_PROMOTIONS")],
            any::<bool>()
        )
            .prop_map(|(t, l, b)| Op::Remote(t, l, b)),
        (0usize..40).prop_map(Op::Bump),
    ]
}

fn delta(kind: ModifyKind) -> (LabelDelta, Option<SnoozeEdit>) {
    let labels = |ids: &[&str]| ids.iter().copied().collect::<LabelSet>();
    match kind {
        ModifyKind::ReturnToInbox => (
            LabelDelta::new(labels(&["INBOX"]), labels(&["TRASH", "SPAM"])).unwrap(),
            Some(SnoozeEdit::Clear),
        ),
        ModifyKind::Snooze => (
            LabelDelta::new(labels(&[]), labels(&["INBOX"])).unwrap(),
            Some(SnoozeEdit::Set(Millis(4_000_000_000_000))),
        ),
        ModifyKind::Done => (
            LabelDelta::new(labels(&[]), labels(&["INBOX", "UNREAD"])).unwrap(),
            None,
        ),
        ModifyKind::Trash => (
            LabelDelta::new(labels(&["TRASH"]), labels(&["INBOX", "SPAM"])).unwrap(),
            None,
        ),
        ModifyKind::Spam => (
            LabelDelta::new(labels(&["SPAM"]), labels(&["INBOX", "TRASH"])).unwrap(),
            None,
        ),
        ModifyKind::Star => (
            LabelDelta::new(labels(&["STARRED"]), labels(&[])).unwrap(),
            None,
        ),
        ModifyKind::Unstar => (
            LabelDelta::new(labels(&[]), labels(&["STARRED"])).unwrap(),
            None,
        ),
        ModifyKind::Read => (
            LabelDelta::new(labels(&[]), labels(&["UNREAD"])).unwrap(),
            None,
        ),
        ModifyKind::Unread => (
            LabelDelta::new(labels(&["UNREAD"]), labels(&[])).unwrap(),
            None,
        ),
        ModifyKind::Label | ModifyKind::RemoveLabel => {
            panic!("no test delta for {kind:?}")
        }
    }
}

#[derive(Default)]
struct Rebuilt {
    lists: HashMap<SubId, Vec<ListEntry>>,
    failures: Vec<String>,
}

fn apply(list: &[ListEntry], removed: &[String], moved: &[ListMove]) -> Vec<ListEntry> {
    let mut out: Vec<ListEntry> = list
        .iter()
        .filter(|e| !removed.contains(&e.key) && !moved.iter().any(|m| m.key == e.key))
        .cloned()
        .collect();
    for m in moved {
        let at = match &m.after {
            None => 0,
            Some(after) => out
                .iter()
                .position(|e| &e.key == after)
                .map_or(out.len(), |p| p + 1),
        };
        out.insert(
            at,
            ListEntry {
                key: m.key.clone(),
                split: m.split,
            },
        );
    }
    out
}

fn fresh(pool: &ReadPool, account: AccountId, view: &View) -> Vec<ListEntry> {
    pool.with(|r| r.threads().list(account, view))
        .unwrap()
        .into_iter()
        .map(|t| ListEntry {
            key: t.thread.to_string(),
            split: t.split,
        })
        .collect()
}

fn stale_lists(
    pool: &ReadPool,
    account: AccountId,
    lists: &HashMap<SubId, Vec<ListEntry>>,
    views: &HashMap<SubId, View>,
) -> Vec<String> {
    lists
        .iter()
        .filter_map(|(sub, list)| {
            let view = views.get(sub)?;
            (fresh(pool, account, view) != *list).then(|| format!("{sub} {view:?}"))
        })
        .collect()
}

fn redated(m: &sm_store::MessageRecord, date: i64) -> NewMessage {
    NewMessage {
        gmail_id: m.gmail_id.clone(),
        date: Millis(date),
        from: m.from.clone(),
        to: m.to.clone(),
        cc: m.cc.clone(),
        bcc: m.bcc.clone(),
        reply_to: m.reply_to.clone(),
        subject: m.subject.clone(),
        snippet: m.snippet.clone(),
        message_id_header: m.message_id_header.clone(),
        in_reply_to: m.in_reply_to.clone(),
        references: m.references.clone(),
        is_list: m.is_list,
        is_draft: m.is_draft,
        sm_draft: m.sm_draft.clone(),
        labels: m.labels.clone(),
        has_attachments: m.has_attachments,
    }
}

type Views = Mutex<HashMap<SubId, View>>;

fn mirror(
    rebuilt: Arc<Mutex<Rebuilt>>,
    views: Arc<Views>,
    pool: Arc<ReadPool>,
    account: AccountId,
) -> Box<dyn FnMut(ChannelMessage) + Send> {
    Box::new(move |msg| {
        let mut st = rebuilt.lock();
        for u in &msg.updates {
            if let SubUpdate::List {
                sub,
                first,
                ids,
                removed,
                moved,
                ..
            } = u
                && views.lock().contains_key(sub)
            {
                let next = if *first {
                    ids.clone().unwrap_or_default()
                } else {
                    let cur = st.lists.get(sub).cloned().unwrap_or_default();
                    apply(&cur, removed, moved)
                };
                st.lists.insert(sub.clone(), next);
            }
        }
        let stale = stale_lists(&pool, account, &st.lists, &views.lock());
        for s in stale {
            st.failures.push(format!("rev {}: {s} differs", msg.rev));
        }
    })
}

fn relabel_latest(
    writer: &CoreWriter,
    thread: ThreadId,
    label: &'static str,
    add: bool,
) -> Pending<(), StoreError> {
    writer.call(Priority::Sync, move |tx, _| {
        let messages = tx.read().messages().for_thread(thread)?;
        if let Some(m) = messages.iter().rev().find(|m| !m.is_draft) {
            let mut labels = m.labels.clone();
            labels.remove(label);
            if add {
                labels.insert(LabelId::from(label));
            }
            tx.messages().set_labels(&[(m.id, labels)])?;
        }
        Ok(())
    })
}

fn redate_latest(writer: &CoreWriter, thread: ThreadId, date: i64) -> Pending<(), StoreError> {
    writer.call(Priority::Sync, move |tx, _| {
        let messages = tx.read().messages().for_thread(thread)?;
        if let Some(m) = messages.iter().rev().find(|m| !m.is_draft) {
            tx.messages().upsert(thread, &redated(m, date))?;
        }
        Ok(())
    })
}

fn modify_action(account: AccountId, kind: ModifyKind, threads: Vec<ThreadId>) -> Action {
    let (delta, edit) = delta(kind);
    Action::Modify(ModifyAction {
        account,
        kind,
        threads,
        add: delta.add().iter().cloned().collect(),
        remove: delta.remove().iter().cloned().collect(),
        snooze: edit.map(|e| match e {
            SnoozeEdit::Set(at) => SnoozeChange::Set(at),
            SnoozeEdit::Clear => SnoozeChange::Clear,
        }),
        undoable: true,
    })
}

struct Replay<'a> {
    app: &'a App,
    account: AccountId,
    threads: &'a [ThreadId],
    views: &'a Views,
    rebuilt: &'a Mutex<Rebuilt>,
    undo: Vec<UndoEntryId>,
    jobs: Vec<Pending<(), StoreError>>,
    n: i64,
    slots: HashMap<usize, SubId>,
    seq: u32,
    newest: i64,
}

impl Replay<'_> {
    fn submit(&mut self, action: Action) -> i64 {
        self.n += 1;
        self.app.actions().submit(vec![NumberedAction {
            n: ActionNo(self.n),
            action,
        }]);
        self.n
    }

    fn pick(&self, i: usize) -> ThreadId {
        self.threads[i % self.threads.len()]
    }

    fn play(&mut self, op: Op) {
        match op {
            Op::Subscribe(slot, view) => self.subscribe(slot, view),
            Op::Unsubscribe(slot) => self.unsubscribe(slot),
            Op::Modify(kind, picked) => {
                let threads = picked.into_iter().map(|i| self.pick(i)).collect();
                let n = self.submit(modify_action(self.account, kind, threads));
                self.undo.push(UndoEntryId(n));
            }
            Op::Undo => {
                if let Some(entry) = self.undo.pop() {
                    let account = self.account;
                    self.submit(Action::Undo { account, entry });
                }
            }
            Op::Remote(i, label, add) => {
                let job = relabel_latest(self.app.writer(), self.pick(i), label, add);
                self.jobs.push(job);
            }
            Op::Bump(i) => {
                let thread = self.pick(i);
                self.newest += 1;
                let job = redate_latest(self.app.writer(), thread, self.newest);
                self.jobs.push(job);
            }
        }
    }

    fn subscribe(&mut self, slot: usize, view: View) {
        if self.slots.contains_key(&slot) {
            return;
        }
        self.seq += 1;
        let sub = SubId(format!("s{}", self.seq));
        self.slots.insert(slot, sub.clone());
        self.views.lock().insert(sub.clone(), view.clone());
        let account = self.account;
        self.submit(Action::Subscribe {
            sub,
            key: SubKey::List { account, view },
        });
    }

    fn unsubscribe(&mut self, slot: usize) {
        if let Some(sub) = self.slots.remove(&slot) {
            self.views.lock().remove(&sub);
            self.rebuilt.lock().lists.remove(&sub);
            self.submit(Action::Unsubscribe { sub });
        }
    }
}

async fn run(ops: Vec<Op>) -> Result<HashMap<SubId, Vec<ListEntry>>, TestCaseError> {
    let h = Harness::start().await;
    let (account, threads) = h.write(seed).await;
    let rebuilt: Arc<Mutex<Rebuilt>> = Arc::default();
    let views: Arc<Views> = Arc::default();
    let pool = Arc::new(
        ReadPool::open(
            &h.dir.path().join(crate::Config::DB_FILE),
            NonZeroUsize::MIN,
        )
        .unwrap(),
    );
    let sink = mirror(
        Arc::clone(&rebuilt),
        Arc::clone(&views),
        Arc::clone(&pool),
        account,
    );
    h.app.lifecycle().connect(sink).await.unwrap();
    let mut replay = Replay {
        app: &h.app,
        account,
        threads: &threads,
        views: &views,
        rebuilt: &rebuilt,
        undo: Vec::new(),
        jobs: Vec::new(),
        n: h.app.actions().last_received().0,
        slots: HashMap::new(),
        seq: 0,
        newest: NEWEST,
    };
    for op in ops {
        replay.play(op);
    }
    let jobs = replay.jobs;
    h.app.actions().drained().await.unwrap();
    for job in jobs {
        job.await.unwrap();
    }
    let st = rebuilt.lock();
    prop_assert!(st.failures.is_empty(), "{:?}", st.failures);
    let stale = stale_lists(&pool, account, &st.lists, &views.lock());
    prop_assert!(stale.is_empty(), "after the last message: {stale:?}");
    Ok(st.lists.clone())
}

fn block_on<T>(fut: impl Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(fut)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, ..ProptestConfig::default() })]
    #[test]
    fn rebuilt_lists_match_fresh_loads(ops in proptest::collection::vec(op(), 1..40)) {
        block_on(run(ops))?;
    }
}

fn sub(name: &str) -> SubId {
    SubId(name.to_owned())
}

#[test]
fn done_and_undo_reach_an_open_inbox_through_patches() {
    let all: Vec<usize> = (0..40).collect();
    let opened = block_on(run(vec![Op::Subscribe(0, View::Inbox)])).unwrap();
    assert_eq!(opened[&sub("s1")].len(), 25);
    let done = block_on(run(vec![
        Op::Subscribe(0, View::Inbox),
        Op::Modify(ModifyKind::Done, all.clone()),
    ]))
    .unwrap();
    assert_eq!(done[&sub("s1")].len(), 0);
    let undone = block_on(run(vec![
        Op::Subscribe(0, View::Inbox),
        Op::Modify(ModifyKind::Done, all),
        Op::Undo,
    ]))
    .unwrap();
    assert_eq!(undone[&sub("s1")].len(), 25);
}

#[test]
fn a_section_change_in_place_reaches_the_mirror() {
    let others: Vec<usize> = (0..40).filter(|&i| i != 3).collect();
    let before = block_on(run(vec![
        Op::Subscribe(0, View::Inbox),
        Op::Modify(ModifyKind::Done, others.clone()),
    ]))
    .unwrap();
    let after = block_on(run(vec![
        Op::Subscribe(0, View::Inbox),
        Op::Modify(ModifyKind::Done, others),
        Op::Remote(3, "CATEGORY_PROMOTIONS", true),
    ]))
    .unwrap();
    assert_eq!(before[&sub("s1")].len(), 1);
    assert_eq!(after[&sub("s1")].len(), 1);
    assert_ne!(before[&sub("s1")][0].split, after[&sub("s1")][0].split);
}

#[test]
fn a_closed_list_then_another_view_rebuilds() {
    block_on(run(vec![
        Op::Subscribe(0, View::Inbox),
        Op::Unsubscribe(0),
        Op::Subscribe(0, View::Done),
    ]))
    .unwrap();
}

#[test]
fn a_snooze_after_a_closed_list_rebuilds() {
    block_on(run(vec![
        Op::Subscribe(0, View::Inbox),
        Op::Unsubscribe(0),
        Op::Modify(ModifyKind::Snooze, vec![0]),
    ]))
    .unwrap();
}

fn hold(h: &Harness) -> (std::sync::mpsc::SyncSender<()>, Pending<(), StoreError>) {
    let (started_tx, started) = std::sync::mpsc::sync_channel(0);
    let (release, release_rx) = std::sync::mpsc::sync_channel::<()>(0);
    let held = h.app.writer().call(Priority::Sync, move |_, _| {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        Ok(())
    });
    started.recv().unwrap();
    (release, held)
}

#[tokio::test]
async fn launch_numbers_continue_after_actions_still_queued() {
    let h = Harness::start().await;
    let last = h.app.actions().last_received().0;
    let (release, held) = hold(&h);
    let old = sub("old");
    h.app.actions().submit(vec![
        NumberedAction {
            n: ActionNo(last + 1),
            action: Action::Subscribe {
                sub: old.clone(),
                key: SubKey::App,
            },
        },
        NumberedAction {
            n: ActionNo(last + 2),
            action: Action::Unsubscribe { sub: old },
        },
    ]);
    let pool = ReadPool::open(
        &h.dir.path().join(crate::Config::DB_FILE),
        NonZeroUsize::MIN,
    )
    .unwrap();
    let stored = pool.with(|r| r.meta().last_action()).unwrap();
    assert_eq!(stored, ActionNo(last), "not committed yet");
    assert_eq!(
        h.app.actions().last_received(),
        ActionNo(last + 2),
        "numbers the writer has queued"
    );
    release.send(()).unwrap();
    held.await.unwrap();
    let launch = h.app.lifecycle().connect(Box::new(|_| {})).await.unwrap();
    assert_eq!(launch.last_action, ActionNo(last + 2));
}

#[tokio::test]
async fn a_first_load_lost_to_a_failed_commit_is_sent_again() {
    let mut h = Harness::start().await;
    h.app
        .writer()
        .call(Priority::Ui, |_, state| {
            state.fail_next_commit = true;
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
    let ns = h
        .act(vec![Action::Subscribe {
            sub: sub("app"),
            key: SubKey::App,
        }])
        .await;
    let n = ActionNo(ns[0]);
    eventually("ack moves past the failed batch", || {
        h.messages.lock().iter().any(|m| m.ack >= n)
    })
    .await;
    let firsts: Vec<SubId> = h
        .take()
        .into_iter()
        .flat_map(|m| m.updates)
        .filter_map(|u| match u {
            SubUpdate::App {
                sub, first: true, ..
            } => Some(sub),
            SubUpdate::List { .. }
            | SubUpdate::Conversation { .. }
            | SubUpdate::Composer { .. }
            | SubUpdate::Account { .. }
            | SubUpdate::App { .. } => None,
        })
        .collect();
    assert_eq!(firsts, [sub("app")], "one first load, after the failure");
}

#[tokio::test]
async fn a_search_opened_in_a_failed_batch_still_settles() {
    let mut h = Harness::start().await;
    let (account, _) = h.write(seed).await;
    h.app
        .writer()
        .call(Priority::Ui, |_, state| {
            state.fail_next_commit = true;
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
    h.act(vec![Action::Subscribe {
        sub: sub("search"),
        key: SubKey::Search {
            account,
            query: "zebra".to_owned(),
        },
    }])
    .await;
    let settled = || {
        h.messages.lock().iter().flat_map(|m| &m.updates).any(|u| {
            matches!(u, SubUpdate::List { sub: s, searching: false, .. } if *s == sub("search"))
        })
    };
    eventually("the search stops searching", settled).await;
}

#[tokio::test]
async fn a_read_stored_in_a_job_that_rolled_back_is_not_the_newest() {
    let h = Harness::start().await;
    let reads = Arc::new(Freshness::default());
    let (older, newer) = (reads.start(), reads.start());
    let failing = Arc::clone(&reads);
    let failed = h
        .app
        .writer()
        .call(Priority::Sync, move |tx, state| {
            assert!(failing.store(newer, state), "the newer read stores first");
            tx.fail_commit();
            Ok::<_, StoreError>(())
        })
        .await;
    assert!(failed.is_err(), "the commit failed");
    let late = Arc::clone(&reads);
    let stored = h
        .app
        .writer()
        .call(Priority::Sync, move |_, state| {
            Ok::<_, StoreError>(late.store(older, state))
        })
        .await
        .unwrap();
    assert!(stored, "the older read's answer is the newest stored");
    let failed = h
        .app
        .writer()
        .call(Priority::Sync, |tx, _| {
            tx.fail_commit();
            Ok::<_, StoreError>(())
        })
        .await;
    assert!(failed.is_err(), "the commit failed");
    let again = Arc::clone(&reads);
    let stored = h
        .app
        .writer()
        .call(Priority::Sync, move |_, state| {
            Ok::<_, StoreError>(again.store(older, state))
        })
        .await
        .unwrap();
    assert!(!stored, "the older read is stored already");
}

#[tokio::test]
async fn a_panic_after_a_commit_still_releases_its_work() {
    let mut h = Harness::start().await;
    let (account, _) = h.write(seed).await;
    h.act(vec![Action::Subscribe {
        sub: sub("app"),
        key: SubKey::App,
    }])
    .await;
    h.app
        .lifecycle()
        .connect(Box::new(|_| panic!("the page's channel is gone")))
        .await
        .unwrap();
    h.write(move |tx| tx.accounts().set_needs_signin(account, true))
        .await;
    eventually("the core goes idle", || h.activity.is_idle()).await;
}

#[tokio::test]
async fn ack_never_moves_back_after_a_failed_batch() {
    let h = Harness::start().await;
    let (account, _) = h.write(seed).await;
    h.app
        .writer()
        .call(Priority::Ui, |_, state| {
            state.fail_next_commit = true;
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
    let start = h.app.actions().last_received().0;
    let batch = |from: i64| -> Vec<NumberedAction> {
        (from..from + 2)
            .map(|n| NumberedAction {
                n: ActionNo(n),
                action: Action::Subscribe {
                    sub: sub(&format!("s{n}")),
                    key: SubKey::Account { account },
                },
            })
            .collect()
    };
    h.app.actions().submit(batch(start + 1));
    h.app.actions().submit(batch(start + 3));
    eventually("the failed batch is rejected", || h.activity.is_idle()).await;
    h.app.actions().drained().await.unwrap();
    let messages = h.take();
    let acks: Vec<i64> = messages.iter().map(|m| m.ack.0).collect();
    assert!(
        acks.windows(2).all(|w| w[0] <= w[1]),
        "ack went back: {acks:?}"
    );
    assert_eq!(acks.last(), Some(&(start + 4)));
    let covering = messages
        .iter()
        .position(|m| m.ack.0 > start)
        .expect("a message covers the failed batch");
    let told: Vec<(i64, &str)> = messages[..=covering]
        .iter()
        .flat_map(|m| &m.rejected)
        .map(|r| (r.n.0, r.message.as_str()))
        .collect();
    let not_saved = "Couldn't save that change. Try again.";
    assert_eq!(told, [(start + 1, not_saved), (start + 2, not_saved)]);
    let all = messages.iter().map(|m| m.rejected.len()).sum::<usize>();
    assert_eq!(all, 2, "each refusal is told once");
}
