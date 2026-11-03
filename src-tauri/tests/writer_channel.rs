//! Writer and channel properties (SPEC Testing): random sequences of UI
//! actions, sync results, subscribes, and closes. After every channel message,
//! a list built from its first load plus patches equals a fresh first load at
//! that `rev`.

#![expect(
    clippy::unwrap_used,
    clippy::unwrap_in_result,
    reason = "Tests fail loudly on purpose."
)]

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use proptest::prelude::*;
use ubermensch_lib::db::queries;
use ubermensch_lib::fixture::{self, Size};
use ubermensch_lib::model::*;
use ubermensch_lib::writer::Writer;

/// The fixture's only account.
const ACCOUNT: AccountId = AccountId(1);

const SIZE: Size = Size {
    threads: 40,
    inbox: 25,
    contacts: 5,
    long_thread: 6,
};

#[derive(Debug, Clone)]
enum Op {
    Subscribe(usize, View),
    Unsubscribe(usize),
    Modify(ModifyKind, Vec<usize>),
    Undo,
    /// A sync result: another client adds or removes a label on a thread's
    /// latest message. `INBOX` moves it in and out of views; a category moves
    /// it between the Important and Other sections.
    Remote(usize, &'static str, bool),
    /// A sync result: a thread's latest message becomes the newest of all, so
    /// the thread moves up past the threads it followed.
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

fn effect(kind: ModifyKind) -> (Vec<String>, Vec<String>, Option<SnoozeChange>) {
    let s = |v: &[&str]| v.iter().map(ToString::to_string).collect::<Vec<_>>();
    match kind {
        ModifyKind::Done => (vec![], s(&["INBOX"]), None),
        ModifyKind::ReturnToInbox => (
            s(&["INBOX"]),
            s(&["TRASH", "SPAM"]),
            Some(SnoozeChange::Clear),
        ),
        ModifyKind::Trash => (s(&["TRASH"]), s(&["INBOX", "SPAM"]), None),
        ModifyKind::Spam => (s(&["SPAM"]), s(&["INBOX", "TRASH"]), None),
        ModifyKind::Star => (s(&["STARRED"]), vec![], None),
        ModifyKind::Unstar => (vec![], s(&["STARRED"]), None),
        ModifyKind::Read => (vec![], s(&["UNREAD"]), None),
        ModifyKind::Unread => (s(&["UNREAD"]), vec![], None),
        ModifyKind::Snooze => (
            vec![],
            s(&["INBOX"]),
            Some(SnoozeChange::Set(4_000_000_000_000)),
        ),
        _ => (vec![], vec![], None),
    }
}

/// The mirror's side: lists rebuilt from first loads and patches.
#[derive(Default)]
struct Rebuilt {
    lists: HashMap<SubId, Vec<ListEntry>>,
    failures: Vec<String>,
}

/// Applies a patch as the UI mirror does (`applyListPatch` in
/// `src/store/mirror-core.ts`), written out here rather than borrowed from the
/// core so a bug shared by the core's diff and its own apply can't cancel out.
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

/// Every open list that differs from a fresh first load.
fn stale_lists(
    conn: &rusqlite::Connection,
    lists: &HashMap<SubId, Vec<ListEntry>>,
    views: &HashMap<SubId, View>,
) -> Vec<String> {
    lists
        .iter()
        .filter_map(|(sub, list)| {
            let v = views.get(sub)?;
            (queries::list_ids(conn, ACCOUNT, v).unwrap() != *list).then(|| format!("{sub} {v:?}"))
        })
        .collect()
}

/// Replays `ops` and returns the lists rebuilt from patches, by subscription id.
#[expect(
    clippy::too_many_lines,
    reason = "The property harness: set up, replay ops, check."
)]
fn run(ops: Vec<Op>) -> Result<HashMap<SubId, Vec<ListEntry>>, TestCaseError> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.db");
    fixture::generate(&path, &SIZE, &["a@example.com"]).unwrap();
    let (etx, _erx) = tokio::sync::mpsc::unbounded_channel();
    let writer = Writer::start(&path, etx).unwrap();

    let state = Arc::new(Mutex::new(Rebuilt::default()));
    let views: Arc<Mutex<HashMap<SubId, View>>> = Arc::default();
    {
        let state = Arc::clone(&state);
        let views = Arc::clone(&views);
        let reader = rusqlite::Connection::open(&path).unwrap();
        writer.set_channel(move |msg: ChannelMessage| {
            let mut st = state.lock();
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
            // Compare every open list with a fresh first load at this rev.
            let diverged = stale_lists(&reader, &st.lists, &views.lock());
            for s in diverged {
                st.failures.push(format!("rev {}: {s} differs", msg.rev));
            }
        });
    }

    let threads: Vec<ThreadId> = {
        let c = rusqlite::Connection::open(&path).unwrap();
        let mut s = c.prepare("SELECT id FROM threads ORDER BY id").unwrap();
        s.query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    let mut n = 0i64;
    let mut slots: HashMap<usize, SubId> = HashMap::new();
    let mut seq = 0;
    let mut next = |a: Action| {
        n += 1;
        NumberedAction { n, action: a }
    };
    for op in ops {
        match op {
            Op::Subscribe(slot, v) => {
                if slots.contains_key(&slot) {
                    continue;
                }
                // Like the UI, never reuse a subscription id.
                seq += 1;
                let sub = SubId(format!("s{seq}"));
                slots.insert(slot, sub.clone());
                views.lock().insert(sub.clone(), v.clone());
                writer.actions(vec![next(Action::Subscribe {
                    sub,
                    key: SubKey::List {
                        account: ACCOUNT,
                        view: v,
                    },
                })]);
            }
            Op::Unsubscribe(slot) => {
                if let Some(sub) = slots.remove(&slot) {
                    // The mirror drops a closed subscription; stop checking it.
                    views.lock().remove(&sub);
                    state.lock().lists.remove(&sub);
                    writer.actions(vec![next(Action::Unsubscribe { sub })]);
                }
            }
            Op::Modify(kind, idx) => {
                let (add, remove, snooze) = effect(kind);
                writer.actions(vec![next(Action::Modify(ModifyAction {
                    account: ACCOUNT,
                    kind,
                    threads: idx.iter().map(|i| threads[i % threads.len()]).collect(),
                    add,
                    remove,
                    snooze,
                    undoable: true,
                }))]);
            }
            Op::Undo => {
                let entry: Option<UndoEntryId> = writer
                    .call_blocking(|ctx| {
                        Ok(ctx
                            .tx
                            .query_row(
                                "SELECT max(id) FROM undo_entries WHERE account_id = 1",
                                [],
                                |r| r.get(0),
                            )
                            .unwrap_or(None))
                    })
                    .unwrap();
                if let Some(entry) = entry {
                    writer.actions(vec![next(Action::Undo {
                        account: ACCOUNT,
                        entry,
                    })]);
                }
            }
            Op::Remote(i, label, add) => {
                let t = threads[i % threads.len()];
                writer.sync(move |ctx| {
                    let msgs = ubermensch_lib::threads::messages(ctx.tx, t)?;
                    if let Some(m) = msgs.last() {
                        let mut labels: Vec<String> =
                            m.labels.iter().filter(|l| *l != label).cloned().collect();
                        if add {
                            labels.push(label.into());
                        }
                        ubermensch_lib::threads::set_message_labels(ctx, m.id, &labels)?;
                        ubermensch_lib::threads::refresh(ctx, t)?;
                    }
                    Ok(())
                });
            }
            Op::Bump(i) => {
                let t = threads[i % threads.len()];
                writer.sync(move |ctx| {
                    let newest: i64 =
                        ctx.tx
                            .query_row("SELECT max(internal_date) FROM messages", [], |r| {
                                r.get(0)
                            })?;
                    let msgs = ubermensch_lib::threads::messages(ctx.tx, t)?;
                    if let Some(m) = msgs.last() {
                        ctx.tx.execute(
                            "UPDATE messages SET internal_date = ?2 WHERE id = ?1",
                            rusqlite::params![m.id, newest + 1],
                        )?;
                        ubermensch_lib::threads::refresh(ctx, t)?;
                    }
                    Ok(())
                });
            }
        }
    }
    // Drain: a blocking call runs after everything queued before it.
    writer.call_blocking(|_| Ok(())).unwrap();
    let st = state.lock();
    prop_assert!(st.failures.is_empty(), "{:?}", st.failures);
    // A change whose patch never came shows only here, after the last message.
    let conn = rusqlite::Connection::open(&path).unwrap();
    let diverged = stale_lists(&conn, &st.lists, &views.lock());
    prop_assert!(diverged.is_empty(), "after the last message: {diverged:?}");
    Ok(st.lists.clone())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, ..ProptestConfig::default() })]
    #[test]
    fn rebuilt_lists_match_fresh_loads(ops in proptest::collection::vec(op(), 1..40)) {
        run(ops)?;
    }
}

/// The property holds trivially if actions never reach the lists. The fixture
/// puts its first 25 threads in Inbox; Done on all 40 empties it, and undo
/// brings the 25 back, all through patches to the open subscription.
#[test]
fn done_and_undo_reach_an_open_inbox_through_patches() {
    let all: Vec<usize> = (0..40).collect();
    let opened = run(vec![Op::Subscribe(0, View::Inbox)]).unwrap();
    assert_eq!(opened["s1"].len(), 25);
    let done = run(vec![
        Op::Subscribe(0, View::Inbox),
        Op::Modify(ModifyKind::Done, all.clone()),
    ])
    .unwrap();
    assert_eq!(done["s1"].len(), 0);
    let undone = run(vec![
        Op::Subscribe(0, View::Inbox),
        Op::Modify(ModifyKind::Done, all),
        Op::Undo,
    ])
    .unwrap();
    assert_eq!(undone["s1"].len(), 25);
}

/// Alone in Inbox, a thread that moves from Important to Other keeps its
/// place, so the patch must carry the new split without any reordering.
#[test]
fn a_section_change_in_place_reaches_the_mirror() {
    let others: Vec<usize> = (0..40).filter(|&i| i != 1).collect();
    let before = run(vec![
        Op::Subscribe(0, View::Inbox),
        Op::Modify(ModifyKind::Done, others.clone()),
    ])
    .unwrap();
    let after = run(vec![
        Op::Subscribe(0, View::Inbox),
        Op::Modify(ModifyKind::Done, others),
        Op::Remote(1, "CATEGORY_PROMOTIONS", true),
    ])
    .unwrap();
    assert_eq!(before["s1"].len(), 1);
    assert_eq!(after["s1"].len(), 1);
    assert_ne!(before["s1"][0].split, after["s1"][0].split);
}

/// A reloaded page continues action numbers from the last one Rust has
/// received, not the last one committed: actions queued behind a slow job
/// would otherwise share numbers with the new page's reset and first
/// subscribe, and the writer would drop the new page's ones.
#[test]
fn launch_numbers_continue_after_actions_still_queued() {
    use ubermensch_lib::engine::{Config, Engine, platform::NoPlatform};

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.db");
    fixture::generate(&path, &SIZE, &["a@example.com"]).unwrap();
    let (etx, _erx) = tokio::sync::mpsc::unbounded_channel();
    let writer = Writer::start(&path, etx).unwrap();
    let engine = Engine::new(
        Arc::new(NoPlatform),
        writer.clone(),
        ubermensch_lib::db::ReadPool::new(&path, 1),
        dir.path().to_path_buf(),
        Config::from_env(),
        Arc::new(ub_gmail::MemoryTokenStore::default()),
    );
    let last = ubermensch_lib::api::launch_info(&engine)
        .unwrap()
        .last_action;

    // A long job holds the writer while the old page's actions arrive.
    let (started_tx, started) = std::sync::mpsc::sync_channel(1);
    let (release, release_rx) = std::sync::mpsc::sync_channel::<()>(1);
    writer.sync(move |_| {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        Ok(())
    });
    started.recv().unwrap();
    let sub = SubId("old".into());
    writer.actions(vec![
        NumberedAction {
            n: last + 1,
            action: Action::Subscribe {
                sub: sub.clone(),
                key: SubKey::App,
            },
        },
        NumberedAction {
            n: last + 2,
            action: Action::Unsubscribe { sub },
        },
    ]);

    let launch = ubermensch_lib::api::launch_info(&engine).unwrap();
    release.send(()).unwrap();
    writer.call_blocking(|_| Ok(())).unwrap();
    assert_eq!(
        launch.last_action,
        last + 2,
        "numbers the writer has queued"
    );
}
