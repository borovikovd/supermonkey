//! Writer and channel properties (SPEC Testing): random sequences of UI
//! actions, sync results, subscribes, and closes. After every channel message,
//! a list built from its first load plus patches equals a fresh first load at
//! that `rev`.

#![allow(clippy::unwrap_used)] // Tests fail loudly on purpose.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use proptest::prelude::*;
use ubermensch_lib::db::queries;
use ubermensch_lib::fixture::{self, Size};
use ubermensch_lib::model::*;
use ubermensch_lib::writer::Writer;

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
    /// A sync result: another client changes a message's labels.
    Remote(usize, bool),
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
        (0usize..40, any::<bool>()).prop_map(|(t, b)| Op::Remote(t, b)),
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
    lists: HashMap<String, Vec<ListEntry>>,
    failures: Vec<String>,
}

fn apply(list: &[ListEntry], removed: &[String], moved: &[ListMove]) -> Vec<ListEntry> {
    ubermensch_lib::subs::diff::apply(list, removed, moved)
}

#[allow(clippy::too_many_lines)] // The property harness: set up, replay ops, check.
fn run(ops: Vec<Op>) -> Result<(), TestCaseError> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.db");
    fixture::generate(&path, &SIZE, &["a@example.com"]).unwrap();
    let (etx, _erx) = tokio::sync::mpsc::unbounded_channel();
    let writer = Writer::start(&path, etx).unwrap();

    let state = Arc::new(Mutex::new(Rebuilt::default()));
    let views: Arc<Mutex<HashMap<String, View>>> = Arc::default();
    {
        let state = state.clone();
        let views = views.clone();
        let reader = Mutex::new(rusqlite::Connection::open(&path).unwrap());
        writer.set_channel(move |msg: ChannelMessage| {
            let mut st = state.lock().unwrap();
            for u in &msg.updates {
                if let SubUpdate::List {
                    sub,
                    first,
                    ids,
                    removed,
                    moved,
                    ..
                } = u
                    && views.lock().unwrap().contains_key(sub)
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
            let conn = reader.lock().unwrap();
            let views = views.lock().unwrap();
            for (sub, list) in st.lists.clone() {
                if let Some(v) = views.get(&sub) {
                    let fresh = queries::list_ids(&conn, 1, v).unwrap();
                    if fresh != list {
                        st.failures
                            .push(format!("rev {}: {sub} {v:?} differs", msg.rev));
                    }
                }
            }
        });
    }

    let threads: Vec<i64> = {
        let c = rusqlite::Connection::open(&path).unwrap();
        let mut s = c.prepare("SELECT id FROM threads ORDER BY id").unwrap();
        s.query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    let mut n = 0i64;
    let mut slots: HashMap<usize, String> = HashMap::new();
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
                let sub = format!("s{seq}");
                slots.insert(slot, sub.clone());
                views.lock().unwrap().insert(sub.clone(), v.clone());
                writer.actions(vec![next(Action::Subscribe {
                    sub,
                    key: SubKey::List {
                        account: 1,
                        view: v,
                    },
                })]);
            }
            Op::Unsubscribe(slot) => {
                if let Some(sub) = slots.remove(&slot) {
                    // The mirror drops a closed subscription; stop checking it.
                    views.lock().unwrap().remove(&sub);
                    state.lock().unwrap().lists.remove(&sub);
                    writer.actions(vec![next(Action::Unsubscribe { sub })]);
                }
            }
            Op::Modify(kind, idx) => {
                let (add, remove, snooze) = effect(kind);
                writer.actions(vec![next(Action::Modify(ModifyAction {
                    account: 1,
                    kind,
                    threads: idx.iter().map(|i| threads[i % threads.len()]).collect(),
                    add,
                    remove,
                    snooze,
                    undoable: true,
                }))]);
            }
            Op::Undo => {
                let entry: Option<i64> = writer
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
                    writer.actions(vec![next(Action::Undo { account: 1, entry })]);
                }
            }
            Op::Remote(i, inbox) => {
                let t = threads[i % threads.len()];
                writer.sync(move |ctx| {
                    let msgs = ubermensch_lib::threads::messages(ctx.tx, t)?;
                    if let Some(m) = msgs.last() {
                        let mut labels: Vec<String> =
                            m.labels.iter().filter(|l| *l != "INBOX").cloned().collect();
                        if inbox {
                            labels.push("INBOX".into());
                        }
                        ubermensch_lib::threads::set_message_labels(ctx, m.id, &labels)?;
                        ubermensch_lib::threads::refresh(ctx, t)?;
                    }
                    Ok(())
                });
            }
        }
    }
    // Drain: a blocking call runs after everything queued before it.
    writer.call_blocking(|_| Ok(())).unwrap();
    let st = state.lock().unwrap();
    prop_assert!(st.failures.is_empty(), "{:?}", st.failures);
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, ..ProptestConfig::default() })]
    #[test]
    fn rebuilt_lists_match_fresh_loads(ops in proptest::collection::vec(op(), 1..40)) {
        run(ops)?;
    }
}
