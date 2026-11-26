use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use proptest::prelude::*;
use supermonkey_lib::core::{
    ChannelSink, Clock, Core, CoreConfig, CoreError, Notification, Platform, diff_list,
};
use supermonkey_lib::domain::{
    AccountId, ActionNo, Addr, DraftId, DraftKind, Millis, ModifyKind, OutboxState, Rev, SplitId,
    SubId, SystemLabel,
};
use supermonkey_lib::gmail::{OAuthConfig, QuotaConfig};
use supermonkey_lib::ipc::{
    Action, AppEvent, ChannelMessage, DraftFields, DraftSave, ListEntry, ListMove, ModifyAction,
    NumberedAction, RejectReason, SnoozeChange, SplitInfo, SubKey, Submitted,
};
use supermonkey_lib::session::{Host, Session};
use supermonkey_lib::store::{NewMessage, Priority as StorePriority, StoreError};

#[derive(Default)]
struct TestPlatform {
    opened_urls: Mutex<Vec<String>>,
    notifications: Mutex<Vec<Notification>>,
    badge: Mutex<u32>,
}

impl Platform for TestPlatform {
    fn open_url(&self, url: &str) -> Result<(), CoreError> {
        self.opened_urls.lock().push(url.to_owned());
        Ok(())
    }

    fn open_file(&self, _path: &Path) -> Result<(), CoreError> {
        Ok(())
    }

    fn notify(&self, notification: Notification) {
        self.notifications.lock().push(notification);
    }

    fn set_badge(&self, count: u32) {
        *self.badge.lock() = count;
    }
}

fn apply_list_patch(old: &[ListEntry], removed: &[String], moved: &[ListMove]) -> Vec<ListEntry> {
    let mut drop_set: HashSet<&str> = removed.iter().map(String::as_str).collect();
    for m in moved {
        drop_set.insert(m.key.as_str());
    }
    let mut out: Vec<ListEntry> = old
        .iter()
        .filter(|e| !drop_set.contains(e.key.as_str()))
        .cloned()
        .collect();
    for m in moved {
        let at = match &m.after {
            None => 0,
            Some(after) => out
                .iter()
                .position(|e| e.key == *after)
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

fn entry_vec_strategy() -> impl Strategy<Value = Vec<ListEntry>> {
    prop::collection::vec((0_u8..20, prop::option::of(1_i64..4)), 0..18).prop_map(|pairs| {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for (k, s) in pairs {
            if seen.insert(k) {
                out.push(ListEntry {
                    key: format!("k{k}"),
                    split: s.map(SplitId),
                });
            }
        }
        out
    })
}

proptest! {
    #[test]
    fn list_patches_always_reconstruct_target_list(
        old in entry_vec_strategy(),
        new in entry_vec_strategy(),
    ) {
        let (removed, moved) = diff_list(&old, &new);
        let patched = apply_list_patch(&old, &removed, &moved);
        prop_assert_eq!(patched, new);
    }
}

#[expect(
    clippy::unwrap_used,
    reason = "integration test harness fails via unwrap on setup errors"
)]
async fn open_test_core(
    start: Millis,
) -> (
    tempfile::TempDir,
    Arc<Core>,
    Arc<Mutex<Vec<ChannelMessage>>>,
) {
    let dir = tempfile::tempdir().unwrap();
    let cfg = CoreConfig {
        data_dir: dir.path().to_path_buf(),
        gmail_base_url: "http://127.0.0.1:9".to_owned(),
        oauth: OAuthConfig::google("cid", "csec"),
        quota: QuotaConfig::default(),
        clock: Clock::manual(start),
    };
    let platform = Box::new(TestPlatform::default());
    let core = Core::open(cfg, platform).unwrap();
    let msgs: Arc<Mutex<Vec<ChannelMessage>>> = Arc::new(Mutex::new(Vec::new()));
    let sink_msgs = Arc::clone(&msgs);
    core.set_channel(Some(ChannelSink::new(move |msg| {
        sink_msgs.lock().push(msg);
    })));
    core.start().await.unwrap();
    (dir, core, msgs)
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "verifies channel ack ordering, rejection reporting, and snooze wake end-to-end"
)]
async fn channel_ack_rejections_and_snooze_wake_work() {
    let (_dir, core, msgs) = open_test_core(Millis(1_000_000)).await;

    let (acct_id, tid) = core
        .writer()
        .call(StorePriority::Ui, |tx, _state| {
            let acct = tx.create_account("ana@acme.test", "Ana")?;
            let tid = tx.place_thread(
                acct.id,
                &supermonkey_lib::domain::GmailThreadId("gt1".to_owned()),
                None,
            )?;
            tx.upsert_message(
                acct.id,
                tid,
                &NewMessage {
                    gmail_id: supermonkey_lib::domain::GmailMessageId("gm1".to_owned()),
                    date: Millis(900_000),
                    from: Addr::named("Bob", "bob@x.test"),
                    to: vec![Addr::new("ana@acme.test")],
                    cc: vec![],
                    bcc: vec![],
                    reply_to: vec![],
                    subject: "Hello".to_owned(),
                    snippet: "Body".to_owned(),
                    message_id_hdr: Some("<m1@x.test>".to_owned()),
                    in_reply_to: None,
                    references: vec![],
                    is_list: false,
                    is_draft: false,
                    sm_draft_id: None,
                    labels: [
                        SystemLabel::Inbox.label_id(),
                        SystemLabel::Unread.label_id(),
                    ]
                    .into_iter()
                    .collect(),
                    has_attachments: false,
                },
            )?;
            tx.derive_thread(tid)?;
            Ok::<_, StoreError>((acct.id, tid))
        })
        .await
        .unwrap();

    let bad_split = Action::SaveSplits {
        account: acct_id,
        splits: vec![SplitInfo {
            id: SplitId(99),
            name: "Bad".to_owned(),
            kind: supermonkey_lib::domain::SplitKind::Custom,
            query: "from:bob -venue".to_owned(),
            enabled: true,
            hide_when_empty: false,
        }],
        deleted: vec![],
    };
    let snooze = Action::Modify(ModifyAction {
        account: acct_id,
        kind: ModifyKind::Snooze,
        threads: vec![tid],
        add: vec![],
        remove: vec![SystemLabel::Inbox.label_id()],
        snooze: Some(SnoozeChange::Set(Millis(1_060_000))),
        undoable: true,
    });

    core.submit(vec![
        Submitted::from(NumberedAction {
            n: ActionNo(1),
            action: Action::Subscribe {
                sub: SubId("app".to_owned()),
                key: SubKey::App,
            },
        }),
        Submitted::from(NumberedAction {
            n: ActionNo(2),
            action: bad_split,
        }),
        Submitted::from(NumberedAction {
            n: ActionNo(3),
            action: snooze,
        }),
    ])
    .await
    .unwrap();

    let last = msgs.lock().last().cloned().unwrap();
    assert_eq!(
        last.ack,
        ActionNo(3),
        "batch advances ack through last action"
    );
    assert_eq!(
        last.rejected.len(),
        1,
        "invalid split rule produces one rejection"
    );
    assert_eq!(
        last.rejected[0].reason,
        RejectReason::SplitRule { split: SplitId(99) }
    );
    assert_eq!(
        last.rejected[0].message,
        "-venue isn't supported in split rules"
    );

    let row = core
        .read_pool()
        .with(|db| db.thread_row(tid))
        .unwrap()
        .unwrap();
    assert!(!row.in_inbox, "snooze removes thread from inbox");
    assert_eq!(row.snooze_until, Some(Millis(1_060_000)));

    assert_eq!(core.clock().advance(60_000), Millis(1_060_000));
    assert!(
        core.wait_idle(Duration::from_secs(2)).await,
        "core settles after snooze wake"
    );
    let after = core
        .read_pool()
        .with(|db| db.thread_row(tid))
        .unwrap()
        .unwrap();
    assert!(after.in_inbox, "due snooze returns thread to inbox");
    assert_eq!(after.snooze_until, None);

    core.stop().await;
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "exercises full draft create, update, send, outbox, and undo lifecycle"
)]
async fn draft_create_edit_send_and_undo_before_delay() {
    let (_dir, core, _msgs) = open_test_core(Millis(2_000_000)).await;
    let acct_id: AccountId = core
        .writer()
        .call(StorePriority::Ui, |tx, _state| {
            Ok::<_, StoreError>(tx.create_account("ana@acme.test", "Ana")?.id)
        })
        .await
        .unwrap();

    let draft_id = DraftId("d-local-1".to_owned());
    core.submit(vec![
        Submitted::from(NumberedAction {
            n: ActionNo(1),
            action: Action::SaveDraft {
                draft: draft_id.clone(),
                save: DraftSave::Create {
                    account: acct_id,
                    kind: DraftKind::New,
                    reply_all: false,
                    thread: None,
                    target: None,
                    started_on_latest: false,
                },
            },
        }),
        Submitted::from(NumberedAction {
            n: ActionNo(2),
            action: Action::SaveDraft {
                draft: draft_id.clone(),
                save: DraftSave::Update {
                    fields: DraftFields {
                        from: Some(Addr::new("ana@acme.test")),
                        to: vec![Addr::new("bob@x.test")],
                        cc: vec![],
                        bcc: vec![],
                        subject: "Hi".to_owned(),
                        body_html: "<p>Hello</p>".to_owned(),
                        attachments: vec![],
                    },
                    rev: Rev(1),
                },
            },
        }),
        Submitted::from(NumberedAction {
            n: ActionNo(3),
            action: Action::SendDraft {
                draft: draft_id.clone(),
            },
        }),
    ])
    .await
    .unwrap();

    let outbox = core
        .read_pool()
        .with(|db| db.outbox_for_account(acct_id))
        .unwrap();
    assert_eq!(outbox.len(), 1, "send creates one outbox row");
    assert_eq!(outbox[0].item.state, OutboxState::Waiting);
    let undo_id = outbox[0].undo_entry.unwrap();
    assert_eq!(
        undo_id.0, 3,
        "undo entry id matches SendDraft action number"
    );

    core.submit(vec![Submitted::from(NumberedAction {
        n: ActionNo(4),
        action: Action::Undo {
            account: acct_id,
            entry: undo_id,
        },
    })])
    .await
    .unwrap();

    let outbox_after = core
        .read_pool()
        .with(|db| db.outbox_for_account(acct_id))
        .unwrap();
    assert!(outbox_after.is_empty(), "undo during delay cancels outbox");
    let restored = core
        .read_pool()
        .with(|db| db.draft(&draft_id))
        .unwrap()
        .unwrap();
    assert!(!restored.in_outbox, "draft is restored out of outbox");
    assert!(restored.lifecycle.is_kept(), "restored draft is kept");
    assert_eq!(SystemLabel::Inbox.label_id(), SystemLabel::Inbox.label_id());

    core.stop().await;
}

struct TestHost {
    exited: Arc<std::sync::atomic::AtomicBool>,
}

impl Host for TestHost {
    fn show_window(&self) {}

    fn emit(&self, _event: AppEvent) {}

    fn exit(&self) {
        self.exited.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

#[tokio::test]
async fn session_focus_and_quit_work_from_non_tokio_thread() {
    let (_dir, core, _msgs) = open_test_core(Millis(3_000_000)).await;
    let exited = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let session = Session::new(
        core,
        Box::new(TestHost {
            exited: Arc::clone(&exited),
        }),
    );

    let s = session.clone();
    std::thread::spawn(move || {
        assert!(s.focus(true).is_none());
        s.request_quit();
    })
    .join()
    .unwrap();

    session.wait_exited().await;
    assert!(
        exited.load(std::sync::atomic::Ordering::SeqCst),
        "request_quit from a non-Tokio thread completes exit cleanly"
    );
}
