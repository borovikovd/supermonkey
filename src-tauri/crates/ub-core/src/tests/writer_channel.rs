//! Writer and channel properties (SPEC Testing), ported from the old
//! `tests/writer_channel.rs`: random sequences of UI actions, mail changes,
//! sync results, subscribes, and closes. After every channel message, a list
//! built from its first load plus patches equals a fresh read of that list
//! at that `rev`.
//!
//! `Modify` and `Undo` are the real UI actions, so the property covers
//! lists changed by the user's own actions as the core makes them.

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Arc;

use parking_lot::Mutex;
use proptest::prelude::*;
use ub_domain::{
    AccountId, ActionNo, LabelDelta, LabelId, LabelSet, Millis, ModifyKind, SnoozeEdit, SubId,
    UndoEntryId, View,
};
use ub_ipc::{
    Action, ListEntry, ListMove, ModifyAction, NumberedAction, SnoozeChange, SubKey, SubUpdate,
};
use ub_store::{NewMessage, Pending, Priority, ReadPool, StoreError};

use super::{Harness, NEWEST, eventually, seed};
use crate::workers::Freshness;

#[derive(Debug, Clone)]
enum Op {
    Subscribe(usize, View),
    Unsubscribe(usize),
    Modify(ModifyKind, Vec<usize>),
    Undo,
    /// A sync result: another client adds or removes a label on a thread's
    /// latest message. `INBOX` moves it in and out of views; a category
    /// moves it between the Important and Other splits.
    Remote(usize, &'static str, bool),
    /// A sync result: a thread's latest message becomes the newest of all,
    /// so the thread moves up past the threads it followed.
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

/// The labels and snooze change of a mail action kind, as the UI sends it.
fn delta(kind: ModifyKind) -> (LabelDelta, Option<SnoozeEdit>) {
    let labels = |ids: &[&str]| ids.iter().copied().collect::<LabelSet>();
    match kind {
        ModifyKind::ReturnToInbox => (
            LabelDelta::new(labels(&["INBOX"]), labels(&["TRASH", "SPAM"])).unwrap(),
            Some(SnoozeEdit::Clear),
        ),
        ModifyKind::Snooze => (
            LabelDelta::preset(kind).unwrap(),
            Some(SnoozeEdit::Set(Millis(4_000_000_000_000))),
        ),
        _ => (LabelDelta::preset(kind).unwrap(), None),
    }
}

/// The mirror's side: lists rebuilt from first loads and patches.
#[derive(Default)]
struct Rebuilt {
    lists: HashMap<SubId, Vec<ListEntry>>,
    failures: Vec<String>,
}

/// Applies a patch as the UI mirror does (`applyListPatch` in
/// `src/store/mirror-core.ts`), written out here rather than borrowed from
/// the core, so a bug shared by the core's diff and its own apply can't
/// cancel out.
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

/// A view's list as a fresh first load would have it, read straight from
/// the store.
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

/// Every open list that differs from a fresh first load.
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

/// The message a thread's latest one becomes when it is redated to `date`.
fn redated(m: &ub_store::MessageRecord, date: i64) -> NewMessage {
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
        ub_draft: m.ub_draft.clone(),
        labels: m.labels.clone(),
        has_attachments: m.has_attachments,
    }
}

/// Replays `ops` and returns the lists rebuilt from patches, by
/// subscription id (`s1`, `s2`, … in subscribe order).
#[expect(
    clippy::too_many_lines,
    reason = "the property harness: set up, replay ops, check"
)]
async fn run(ops: Vec<Op>) -> Result<HashMap<SubId, Vec<ListEntry>>, TestCaseError> {
    let h = Harness::start().await;
    let (account, threads) = h.write(seed).await;
    let rebuilt: Arc<Mutex<Rebuilt>> = Arc::default();
    let views: Arc<Mutex<HashMap<SubId, View>>> = Arc::default();
    let pool = Arc::new(
        ReadPool::open(
            &h.dir.path().join(crate::Config::DB_FILE),
            NonZeroUsize::MIN,
        )
        .unwrap(),
    );
    {
        let (rebuilt, views, pool) = (Arc::clone(&rebuilt), Arc::clone(&views), Arc::clone(&pool));
        h.app
            .lifecycle()
            .connect(Box::new(move |msg| {
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
                // Every open list against a fresh first load at this rev.
                let stale = stale_lists(&pool, account, &st.lists, &views.lock());
                for s in stale {
                    st.failures.push(format!("rev {}: {s} differs", msg.rev));
                }
            }))
            .await
            .unwrap();
    }
    let writer = h.app.writer();
    // The undo entries made so far, newest last: each is its action's number.
    let mut undo: Vec<UndoEntryId> = Vec::new();
    let mut jobs: Vec<Pending<(), StoreError>> = Vec::new();
    let mut n = h.app.actions().last_received().0;
    let mut slots: HashMap<usize, SubId> = HashMap::new();
    let mut seq = 0;
    let mut newest = NEWEST;
    let mut submit = |action: Action| {
        n += 1;
        h.app.actions().submit(vec![NumberedAction {
            n: ActionNo(n),
            action,
        }]);
        n
    };
    let pick = |i: usize| threads[i % threads.len()];
    for op in ops {
        match op {
            Op::Subscribe(slot, view) => {
                if slots.contains_key(&slot) {
                    continue;
                }
                // Like the UI, never reuse a subscription id.
                seq += 1;
                let sub = SubId(format!("s{seq}"));
                slots.insert(slot, sub.clone());
                views.lock().insert(sub.clone(), view.clone());
                submit(Action::Subscribe {
                    sub,
                    key: SubKey::List { account, view },
                });
            }
            Op::Unsubscribe(slot) => {
                if let Some(sub) = slots.remove(&slot) {
                    // The mirror drops a closed subscription; stop checking it.
                    views.lock().remove(&sub);
                    rebuilt.lock().lists.remove(&sub);
                    submit(Action::Unsubscribe { sub });
                }
            }
            Op::Modify(kind, picked) => {
                let (delta, edit) = delta(kind);
                let n = submit(Action::Modify(ModifyAction {
                    account,
                    kind,
                    threads: picked.into_iter().map(pick).collect(),
                    add: delta.add().iter().cloned().collect(),
                    remove: delta.remove().iter().cloned().collect(),
                    snooze: edit.map(|e| match e {
                        SnoozeEdit::Set(at) => SnoozeChange::Set(at),
                        SnoozeEdit::Clear => SnoozeChange::Clear,
                    }),
                    undoable: true,
                }));
                undo.push(UndoEntryId(n));
            }
            Op::Undo => {
                if let Some(entry) = undo.pop() {
                    submit(Action::Undo { account, entry });
                }
            }
            Op::Remote(i, label, add) => {
                let thread = pick(i);
                jobs.push(writer.call(Priority::Sync, move |tx, _| {
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
                }));
            }
            Op::Bump(i) => {
                let thread = pick(i);
                newest += 1;
                let date = newest;
                jobs.push(writer.call(Priority::Sync, move |tx, _| {
                    let messages = tx.read().messages().for_thread(thread)?;
                    if let Some(m) = messages.iter().rev().find(|m| !m.is_draft) {
                        tx.messages().upsert(thread, &redated(m, date))?;
                    }
                    Ok(())
                }));
            }
        }
    }
    // A background job runs after everything queued before it.
    h.app.actions().drained().await.unwrap();
    for job in jobs {
        job.await.unwrap();
    }
    let st = rebuilt.lock();
    prop_assert!(st.failures.is_empty(), "{:?}", st.failures);
    // A change whose patch never came shows only here, after the last message.
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

/// The property holds trivially if changes never reach the lists. The
/// fixture puts its first 25 threads in Inbox; Done on all 40 empties it,
/// and undo brings the 25 back, all through patches to the open list.
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

/// Alone in Inbox, a thread that moves from Important to Other keeps its
/// place, so the patch must carry the new split without any reordering.
/// (Thread 3 has one message, from someone else, so the category lands on
/// the message routing reads.)
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

/// Shrunk cases proptest found against the old core
/// (`writer_channel.proptest-regressions`), kept as plain tests: the
/// strategy changed, so the stored seeds no longer replay them.
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

/// Holds the writer busy until the returned sender sends.
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

/// A reloaded page continues action numbers from the last one Rust has
/// received, not the last one committed: actions queued behind a slow job
/// would otherwise share numbers with the new page's reset and first
/// subscribe, and the writer would drop the new page's ones.
#[tokio::test]
async fn launch_numbers_continue_after_actions_still_queued() {
    let h = Harness::start().await;
    let last = h.app.actions().last_received().0;
    // A long job holds the writer while the old page's actions arrive.
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

/// A commit that fails must not leave the subscriptions believing its
/// messages went out: a subscription whose first load was lost gets one
/// with the next commit, instead of waiting for it forever. And `ack`
/// still moves past the failed batch, so the mirror drops its actions.
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
            _ => None,
        })
        .collect();
    assert_eq!(firsts, [sub("app")], "one first load, after the failure");
}

/// A batch whose commit fails keeps the subscriptions it opened, so the
/// work they asked for must go out too. A search whose Gmail listing was
/// dropped with the failed commit would say it is searching forever.
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
    // The account has no worker, so its listing settles at once, empty.
    let settled = || {
        h.messages.lock().iter().flat_map(|m| &m.updates).any(|u| {
            matches!(u, SubUpdate::List { sub: s, searching: false, .. } if *s == sub("search"))
        })
    };
    eventually("the search stops searching", settled).await;
}

/// Two reads of the labels answer out of order. The newer one's store rolls
/// back: its answer was never stored, so the older one's, stored next, must
/// not be dropped as stale.
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
    // A later job that rolls back leaves the committed store alone.
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

/// A commit whose updates can't be sent (the sink panicked) still releases
/// its effects and lets the core go idle: `settle()` would wait forever
/// otherwise, and the effects would leave with some later commit.
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
    // A commit with a patch to send, and an effect (the badge).
    h.write(move |tx| tx.accounts().set_needs_signin(account, true))
        .await;
    eventually("the core goes idle", || h.activity.is_idle()).await;
}

/// A batch whose commit failed is rejected by a later job that moves `ack`
/// past it. A batch queued behind the failed one commits first, with higher
/// numbers: `ack` must not then go back to the failed batch's last number.
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
    // The second batch is queued before the first one's failure is seen.
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
    // The failed batch's actions are refused, and the page hears so no later
    // than the message whose `ack` first covers them: otherwise it would
    // take them for applied.
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
