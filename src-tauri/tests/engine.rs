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
    AccountId, ActionNo, Addr, Millis, ModifyKind, SplitId, SubId, SystemLabel,
};
use supermonkey_lib::gmail::{OAuthConfig, QuotaConfig};
use supermonkey_lib::ipc::{
    Action, AppEvent, ChannelMessage, ListEntry, ListMove, ModifyAction, NumberedAction,
    RejectReason, SnoozeChange, SplitInfo, SubKey, SubUpdate, Submitted,
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
async fn open_core_in(
    data_dir: &Path,
    start: Millis,
) -> (Arc<Core>, Arc<Mutex<Vec<ChannelMessage>>>) {
    let cfg = CoreConfig {
        data_dir: data_dir.to_path_buf(),
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
    (core, msgs)
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
    let (core, msgs) = open_core_in(dir.path(), start).await;
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
                    has_attachments: Some(false),
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

fn sub_app(n: i64, sub: &str) -> Submitted {
    Submitted::from(NumberedAction {
        n: ActionNo(n),
        action: Action::Subscribe {
            sub: SubId(sub.to_owned()),
            key: SubKey::App,
        },
    })
}

#[tokio::test]
async fn action_numbers_persist_across_reload_and_ack_never_moves_back() {
    let dir = tempfile::tempdir().unwrap();
    let (core, _msgs) = open_core_in(dir.path(), Millis(2_000_000)).await;
    core.submit(vec![
        sub_app(1, "app"),
        Submitted::from(NumberedAction {
            n: ActionNo(2),
            action: Action::Unsubscribe {
                sub: SubId("app".to_owned()),
            },
        }),
    ])
    .await
    .unwrap();
    core.stop().await;

    let (reloaded, msgs) = open_core_in(dir.path(), Millis(2_001_000)).await;
    let info = reloaded.connect().unwrap();
    assert_eq!(
        info.last_action,
        ActionNo(2),
        "reloaded core reports prior last_action"
    );

    reloaded.submit(vec![sub_app(1, "stale")]).await.unwrap();
    assert!(
        msgs.lock().is_empty(),
        "stale action number is deduplicated without moving ack backward"
    );

    reloaded.submit(vec![sub_app(3, "fresh")]).await.unwrap();
    let last = msgs.lock().last().cloned().unwrap();
    assert_eq!(
        last.ack,
        ActionNo(3),
        "next action advances ack monotonically"
    );
    assert!(
        matches!(
            last.updates.as_slice(),
            [SubUpdate::App { first: true, .. }]
        ),
        "new subscription emits first load"
    );
    reloaded.stop().await;
}

#[tokio::test]
async fn failed_commit_and_panicking_job_leave_core_consistent_and_idle() {
    let (_dir, core, msgs) = open_test_core(Millis(2_500_000)).await;

    let failed = core
        .writer()
        .call(StorePriority::Ui, |tx, _state| {
            let _acct: AccountId = tx.create_account("ghost@acme.test", "Ghost")?.id;
            tx.fail_commit();
            Ok::<_, StoreError>(())
        })
        .await;
    assert!(failed.is_err(), "injected commit failure returns error");

    let panicked: Result<(), StoreError> = core
        .writer()
        .call(StorePriority::Sync, |_tx, _state| panic!("writer boom"))
        .await;
    assert!(
        matches!(panicked, Err(StoreError::JobPanicked(_))),
        "panicking writer job surfaces JobPanicked"
    );
    assert!(
        core.wait_idle(Duration::from_secs(2)).await,
        "core returns to idle after failed and panicking commits"
    );

    core.submit(vec![sub_app(1, "app")]).await.unwrap();
    let last = msgs.lock().last().cloned().unwrap();
    assert_eq!(
        last.ack,
        ActionNo(1),
        "writer keeps serving after rollback/panic"
    );
    match last.updates.as_slice() {
        [
            SubUpdate::App {
                first: true, state, ..
            },
        ] => {
            assert!(
                state.accounts.is_empty(),
                "rolled-back account never appears in snapshot"
            );
        }
        other => panic!("expected first App snapshot, got {other:?}"),
    }
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

struct ChaosRng(u64);

impl ChaosRng {
    const fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn pick(&mut self, len: usize) -> usize {
        usize::try_from(self.next_u64() % u64::try_from(len.max(1)).unwrap_or(1)).unwrap_or(0)
    }
}

struct ChaosEnv {
    dir: tempfile::TempDir,
    fake: sm_fake_gmail::FakeGmail,
    user: sm_fake_gmail::FakeUser,
    account: AccountId,
    core: Arc<Core>,
    next_n: i64,
}

impl ChaosEnv {
    fn config(data_dir: &Path, fake: &sm_fake_gmail::FakeGmail, clock: Clock) -> CoreConfig {
        CoreConfig {
            data_dir: data_dir.to_path_buf(),
            gmail_base_url: fake.base_url(),
            oauth: OAuthConfig {
                client_id: "chaos-client".to_owned(),
                client_secret: "chaos-secret".to_owned(),
                auth_url: fake.auth_url(),
                token_url: fake.token_url(),
                revoke_url: fake.revoke_url(),
            },
            quota: QuotaConfig::for_budget(supermonkey_lib::gmail::Units(60_000)),
            clock,
        }
    }

    async fn open_core(
        data_dir: &Path,
        fake: &sm_fake_gmail::FakeGmail,
        now: Millis,
    ) -> Result<Arc<Core>, CoreError> {
        let core = Core::open(
            Self::config(data_dir, fake, Clock::manual(now)),
            Box::new(TestPlatform::default()),
        )?;
        core.start().await?;
        Ok(core)
    }

    async fn seed() -> Result<(Self, sm_fake_gmail::AddedMessage), Box<dyn std::error::Error>> {
        use supermonkey_lib::gmail::{RefreshToken, TokenFile};
        use supermonkey_lib::store::with_write_tx;

        let dir = tempfile::tempdir()?;
        let fake = sm_fake_gmail::FakeGmail::start();
        let user = fake.add_user("ana@acme.test");
        user.set_display_name("Ana");

        let base_ms = 1_762_000_000_000_i64;
        let mut att_msg = sm_fake_gmail::NewMessage::inbox(
            "Carol <carol@x.test>",
            "With PDF",
            "Check the attached file.",
        );
        att_msg.internal_date = Some(u64::try_from(base_ms - 10_000).unwrap_or(0));
        att_msg.attachments.push(sm_fake_gmail::NewAttachment {
            filename: "plan.pdf".to_owned(),
            mime_type: "application/pdf".to_owned(),
            data: b"%PDF-1.4".to_vec(),
            content_id: None,
            inline: false,
            served_inline: false,
        });
        let att_added = user.add_message(att_msg);
        for i in 0..4_i64 {
            let mut m = sm_fake_gmail::NewMessage::inbox(
                "Bob <bob@x.test>",
                &format!("Seed {i}"),
                "Seed body.",
            );
            m.internal_date = Some(u64::try_from(base_ms - i * 1_000).unwrap_or(0));
            drop(user.add_message(m));
        }

        TokenFile::new(dir.path().join("tokens.json"))
            .store(user.email(), &RefreshToken::new(user.refresh_token()))?;
        let account = with_write_tx(&dir.path().join("mail.db"), |tx| {
            Ok::<_, StoreError>(tx.create_account("ana@acme.test", "Ana")?.id)
        })?;
        let core = Self::open_core(dir.path(), &fake, Millis(base_ms)).await?;
        assert!(
            core.wait_idle(Duration::from_secs(10)).await,
            "initial sync settles"
        );
        Ok((
            Self {
                dir,
                fake,
                user,
                account,
                core,
                next_n: 0,
            },
            att_added,
        ))
    }

    async fn submit(&mut self, action: Action) -> Result<(), CoreError> {
        self.next_n += 1;
        self.core
            .submit(vec![Submitted::from(NumberedAction {
                n: ActionNo(self.next_n),
                action,
            })])
            .await?;
        Ok(())
    }

    async fn settle(&self) {
        self.fake.clear_faults();
        self.fake.set_offline(false).await;
        self.core.probe_network().await;
        for _ in 0..4 {
            let now = self.core.clock().advance(7_200_000);
            assert!(now.0 > 0, "manual clock advances monotonically");
            self.core.wake_retrying();
            self.core.poll_all();
            assert!(
                self.core.wait_idle(Duration::from_secs(10)).await,
                "chaos core must settle cleanly"
            );
        }
    }

    async fn run_step(
        &mut self,
        rng: &mut ChaosRng,
        step: usize,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let threads = self
            .core
            .read_pool()
            .with(|db| db.thread_sync_states(self.account))?;
        match rng.pick(7) {
            0 | 1 if !threads.is_empty() => {
                let tid = threads[rng.pick(threads.len())].id;
                let action = random_modify_action(rng, self.account, tid, self.core.clock().now());
                self.submit(action).await?;
            }
            2 => {
                let top_undo = self
                    .core
                    .read_pool()
                    .with(|db| db.undo_entries(self.account))?
                    .into_iter()
                    .next();
                if let Some(entry) = top_undo {
                    self.submit(Action::Undo {
                        account: self.account,
                        entry: entry.id,
                    })
                    .await?;
                }
            }
            3 => self.deliver_remote(step),
            4 => self.mutate_remote(rng),
            5 => self.inject_fault(rng),
            _ => self.perturb_network_or_restart(rng).await?,
        }
        Ok(())
    }

    fn deliver_remote(&self, step: usize) {
        let mut msg = sm_fake_gmail::NewMessage::inbox(
            "Remote <rem@x.test>",
            &format!("Remote msg {step}"),
            "Body from remote.",
        );
        msg.internal_date = Some(u64::try_from(self.core.clock().now().0).unwrap_or(0));
        drop(self.user.add_message(msg));
        self.core.poll_all();
    }

    fn mutate_remote(&self, rng: &mut ChaosRng) {
        let remote = self.user.messages();
        if let Some(m) = remote
            .iter()
            .find(|m| !m.label_ids.iter().any(|l| l == "DRAFT"))
        {
            let add = if rng.pick(2) == 0 {
                ["STARRED"]
            } else {
                ["UNREAD"]
            };
            let empty: &[&str] = &[];
            assert!(
                self.user.modify_labels(&m.id, &add, empty),
                "remote label modification applies to existing fake message"
            );
            if rng.pick(3) == 0 {
                self.user.expire_history();
            }
            self.core.poll_all();
        }
    }

    fn inject_fault(&self, rng: &mut ChaosRng) {
        let apis = [
            "threads.get",
            "history.list",
            "messages.batchModify",
            "drafts.list",
        ];
        let api = apis[rng.pick(apis.len())];
        let fault_id = self.fake.inject(
            sm_fake_gmail::Matcher::api(api).once(),
            sm_fake_gmail::Fault::Status {
                status: 503,
                reason: "backendError".to_owned(),
                retry_after_secs: None,
            },
        );
        assert!(fault_id > 0, "injected fault gets a positive id");
        self.core.poll_all();
    }

    async fn perturb_network_or_restart(
        &mut self,
        rng: &mut ChaosRng,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let now = self.core.clock().advance(60_000);
        if rng.pick(2) == 0 {
            self.fake.set_offline(true).await;
            self.core.probe_network().await;
            self.fake.set_offline(false).await;
            self.core.probe_network().await;
        } else {
            let stopped = tokio::time::timeout(Duration::from_secs(5), self.core.stop()).await;
            assert!(
                stopped.is_ok(),
                "mid-run Core::stop finishes within deadline"
            );
            self.core = Self::open_core(self.dir.path(), &self.fake, now).await?;
            let info = self.core.connect()?;
            assert_eq!(
                info.last_action,
                ActionNo(self.next_n),
                "reopened core preserves last_action"
            );
        }
        Ok(())
    }

    fn assert_converged(&self) -> Result<(), StoreError> {
        use supermonkey_lib::domain::OpQueue;

        let (mail_op, draft_op, outbox) = self.core.read_pool().with(|db| {
            Ok::<_, StoreError>((
                db.next_op(self.account, OpQueue::Mail)?,
                db.next_op(self.account, OpQueue::Drafts)?,
                db.outbox_for_account(self.account)?,
            ))
        })?;
        assert!(mail_op.is_none(), "no stuck mail ops after settle");
        assert!(draft_op.is_none(), "no stuck draft ops after settle");
        assert!(outbox.is_empty(), "no stuck outbox rows after settle");

        let mut by_thread: std::collections::HashMap<String, Vec<sm_fake_gmail::FakeMessage>> =
            std::collections::HashMap::new();
        for msg in self.user.messages() {
            if !msg.label_ids.iter().any(|l| l == "DRAFT") {
                by_thread
                    .entry(msg.thread_id.clone())
                    .or_default()
                    .push(msg);
            }
        }
        for (gtid, msgs) in by_thread {
            self.assert_thread_converged(&gtid, &msgs)?;
        }
        Ok(())
    }

    fn assert_thread_converged(
        &self,
        gtid: &str,
        msgs: &[sm_fake_gmail::FakeMessage],
    ) -> Result<(), StoreError> {
        use supermonkey_lib::domain::GmailThreadId;

        let live: Vec<&sm_fake_gmail::FakeMessage> = msgs
            .iter()
            .filter(|m| !m.label_ids.iter().any(|l| l == "TRASH" || l == "SPAM"))
            .collect();
        let has_live = !live.is_empty();
        let expected_inbox = has_live
            && live
                .iter()
                .any(|m| m.label_ids.iter().any(|l| l == "INBOX"));
        let expected_trash = msgs
            .iter()
            .any(|m| m.label_ids.iter().any(|l| l == "TRASH"));
        let counted: Vec<&sm_fake_gmail::FakeMessage> = if has_live {
            live
        } else {
            msgs.iter().collect()
        };
        let expected_unread = counted
            .iter()
            .any(|m| m.label_ids.iter().any(|l| l == "UNREAD"));
        let expected_starred = has_live
            && counted
                .iter()
                .any(|m| m.label_ids.iter().any(|l| l == "STARRED"));

        let row = self.core.read_pool().with(|db| {
            let tid = db.thread_by_gmail(self.account, &GmailThreadId(gtid.to_owned()))?;
            tid.map(|id| db.thread_row(id))
                .transpose()
                .map(Option::flatten)
        })?;
        let acct_row = self.core.read_pool().with(|db| db.account(self.account))?;
        let labels_summary: Vec<(&str, &[String])> = msgs
            .iter()
            .map(|m| (m.id.as_str(), m.label_ids.as_slice()))
            .collect();
        assert!(
            row.is_some(),
            "thread {gtid} missing locally after convergence (msgs={labels_summary:?}, acct_history={:?}, acct_connected={:?})",
            acct_row.as_ref().and_then(|a| a.history_id),
            acct_row.as_ref().map(|a| a.connected),
        );
        if let Some(row) = row {
            assert_eq!(row.has_live, has_live, "has_live converged for {gtid}");
            assert_eq!(row.in_inbox, expected_inbox, "inbox converged for {gtid}");
            assert_eq!(
                row.labels.contains(&SystemLabel::Trash.label_id()),
                expected_trash,
                "trash converged for {gtid}"
            );
            assert_eq!(row.unread, expected_unread, "unread converged for {gtid}");
            assert_eq!(
                row.labels.contains(&SystemLabel::Starred.label_id()),
                expected_starred,
                "starred converged for {gtid}"
            );
        }
        Ok(())
    }

    async fn exercise_draft_and_metadata(
        &mut self,
        seed: u64,
        att_added: &sm_fake_gmail::AddedMessage,
    ) -> Result<(), Box<dyn std::error::Error>> {
        use supermonkey_lib::domain::{DraftId, DraftKind, GmailThreadId};
        use supermonkey_lib::ipc::DraftSave;

        let draft_id = DraftId(format!("chaos-draft-{seed}"));
        self.submit(Action::SaveDraft {
            draft: draft_id.clone(),
            save: DraftSave::Create {
                account: self.account,
                kind: DraftKind::New,
                reply_all: false,
                thread: None,
                target: None,
                started_on_latest: false,
            },
        })
        .await?;
        self.submit(Action::DiscardDraft { draft: draft_id })
            .await?;

        self.settle().await;
        self.assert_converged()?;

        let empty: &[&str] = &[];
        assert!(
            self.user.modify_labels(&att_added.id, &["STARRED"], empty),
            "remote star modification applies to attachment fixture message"
        );
        self.settle().await;
        let att_row = self.core.read_pool().with(|db| {
            let tid =
                db.thread_by_gmail(self.account, &GmailThreadId(att_added.thread_id.clone()))?;
            tid.map(|id| db.thread_row(id))
                .transpose()
                .map(Option::flatten)
        })?;
        assert!(
            att_row.is_some_and(|r| r.has_attachments),
            "metadata refetch preserves has_attachments from prior full fetch"
        );
        Ok(())
    }

    async fn remove_account_and_verify_clean(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.submit(Action::RemoveAccount {
            account: self.account,
        })
        .await?;
        assert!(
            self.core.wait_idle(Duration::from_secs(5)).await,
            "account removal settles"
        );
        #[expect(
            clippy::redundant_closure_for_method_calls,
            reason = "ReadPool::with requires a higher-ranked closure over Db<'_>"
        )]
        let remaining = self.core.read_pool().with(|db| db.accounts())?;
        assert!(remaining.is_empty(), "removed account deleted from SQLite");
        assert!(
            !dir_has_files(&self.dir.path().join("attachments")),
            "no orphan attachment files after account removal"
        );
        assert!(
            !dir_has_files(&self.dir.path().join("drafts")),
            "no orphan draft files after account removal"
        );
        let stopped = tokio::time::timeout(Duration::from_secs(5), self.core.stop()).await;
        assert!(stopped.is_ok(), "final Core::stop finishes within deadline");
        Ok(())
    }
}

fn random_modify_action(
    rng: &mut ChaosRng,
    account: AccountId,
    tid: supermonkey_lib::domain::ThreadId,
    now: Millis,
) -> Action {
    let inbox = SystemLabel::Inbox.label_id();
    let star = SystemLabel::Starred.label_id();
    let unread = SystemLabel::Unread.label_id();
    let trash = SystemLabel::Trash.label_id();
    let (kind, add, remove, snooze) = match rng.pick(8) {
        0 => (ModifyKind::Done, vec![], vec![inbox], None),
        1 => (ModifyKind::ReturnToInbox, vec![inbox], vec![trash], None),
        2 => (ModifyKind::Star, vec![star], vec![], None),
        3 => (ModifyKind::Unstar, vec![], vec![star], None),
        4 => (ModifyKind::Read, vec![], vec![unread], None),
        5 => (ModifyKind::Unread, vec![unread], vec![], None),
        6 => (ModifyKind::Trash, vec![trash], vec![inbox], None),
        _ => (
            ModifyKind::Snooze,
            vec![],
            vec![inbox],
            Some(SnoozeChange::Set(Millis(now.0 + 3_600_000))),
        ),
    };
    Action::Modify(ModifyAction {
        account,
        kind,
        threads: vec![tid],
        add,
        remove,
        snooze,
        undoable: true,
    })
}

fn dir_has_files(path: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(path) else {
        return false;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_file() || (p.is_dir() && dir_has_files(&p)) {
            return true;
        }
    }
    false
}

#[tokio::test]
async fn stateful_chaos_converges_local_state_to_gmail_and_cleans_up() {
    for seed in [0xA11C_E001_u64, 0xB0B0_0002, 0xCA05_0003] {
        let mut rng = ChaosRng(seed);
        let (mut env, att_added) = ChaosEnv::seed().await.unwrap();
        for step in 0..16 {
            env.run_step(&mut rng, step).await.unwrap();
        }
        env.exercise_draft_and_metadata(seed, &att_added)
            .await
            .unwrap();
        env.remove_account_and_verify_clean().await.unwrap();
    }
}

fn missing_body_threads(
    core: &Core,
    account: AccountId,
) -> Result<Vec<supermonkey_lib::domain::GmailThreadId>, StoreError> {
    core.read_pool().with(|db| {
        let states = db.thread_sync_states(account)?;
        let tids: Vec<supermonkey_lib::domain::ThreadId> = states.iter().map(|s| s.id).collect();
        let needing = db.threads_needing_bodies(&tids)?;
        Ok(states
            .into_iter()
            .filter_map(|s| needing.contains(&s.id).then_some(s.gmail_id))
            .collect())
    })
}

async fn settle_with_retries(core: &Core) {
    for _ in 0..4 {
        assert!(
            core.clock().advance(120_000).0 > 0,
            "manual clock advances during retry settle"
        );
        core.wake_retrying();
        core.poll_all();
        assert!(
            core.wait_idle(Duration::from_secs(10)).await,
            "core settles after retry"
        );
    }
}

fn seed_body_recovery_fixture(
    dir: &Path,
    fake: &sm_fake_gmail::FakeGmail,
    base_ms: i64,
) -> Result<(sm_fake_gmail::FakeUser, AccountId), Box<dyn std::error::Error>> {
    use supermonkey_lib::gmail::{RefreshToken, TokenFile};
    use supermonkey_lib::store::with_write_tx;

    let user = fake.add_user("ana@acme.test");
    user.set_display_name("Ana");
    for i in 0..8_i64 {
        let mut m = sm_fake_gmail::NewMessage::inbox(
            "Bob <bob@x.test>",
            &format!("Inbox {i}"),
            &format!("Inbox body {i}"),
        );
        m.internal_date = Some(u64::try_from(base_ms - i * 1_000).unwrap_or(0));
        drop(user.add_message(m));
    }
    for i in 0..2_i64 {
        let mut m = sm_fake_gmail::NewMessage::inbox(
            "Star <star@x.test>",
            &format!("Starred {i}"),
            &format!("Starred body {i}"),
        );
        m.labels = vec!["STARRED".to_owned()];
        m.internal_date = Some(u64::try_from(base_ms - 50_000 - i * 1_000).unwrap_or(0));
        drop(user.add_message(m));
    }
    TokenFile::new(dir.join("tokens.json"))
        .store(user.email(), &RefreshToken::new(user.refresh_token()))?;
    let account = with_write_tx(&dir.join("mail.db"), |tx| {
        Ok::<_, StoreError>(tx.create_account("ana@acme.test", "Ana")?.id)
    })?;
    Ok((user, account))
}

fn inject_once_503(fake: &sm_fake_gmail::FakeGmail, api: &str) {
    let fault_id = fake.inject(
        sm_fake_gmail::Matcher::api(api).once(),
        sm_fake_gmail::Fault::Status {
            status: 503,
            reason: "backendError".to_owned(),
            retry_after_secs: None,
        },
    );
    assert!(fault_id > 0, "injected transient 503 for {api}");
}

#[tokio::test]
async fn sync_fetches_missing_bodies_after_transient_thread_failure_resync_and_restart() {
    let base_ms = 1_762_000_000_000_i64;
    let dir = tempfile::tempdir().unwrap();
    let fake = sm_fake_gmail::FakeGmail::start();
    let (user, account) = seed_body_recovery_fixture(dir.path(), &fake, base_ms).unwrap();

    inject_once_503(&fake, "threads.get");
    let core = ChaosEnv::open_core(dir.path(), &fake, Millis(base_ms))
        .await
        .unwrap();
    assert!(
        core.wait_idle(Duration::from_secs(10)).await,
        "initial failed attempt settles"
    );
    settle_with_retries(&core).await;
    assert_eq!(
        missing_body_threads(&core, account).unwrap(),
        Vec::<supermonkey_lib::domain::GmailThreadId>::new(),
        "every inbox and starred thread gets a body after a transient first-sync failure"
    );

    user.expire_history();
    for i in 0..4_i64 {
        let mut m = sm_fake_gmail::NewMessage::inbox(
            "Resync <resync@x.test>",
            &format!("Resync {i}"),
            &format!("Resync body {i}"),
        );
        m.internal_date = Some(u64::try_from(base_ms + (i + 1) * 1_000).unwrap_or(0));
        drop(user.add_message(m));
    }
    inject_once_503(&fake, "threads.get");
    core.poll_all();
    assert!(
        core.wait_idle(Duration::from_secs(10)).await,
        "initial failed resync settles"
    );
    settle_with_retries(&core).await;
    assert_eq!(
        missing_body_threads(&core, account).unwrap(),
        Vec::<supermonkey_lib::domain::GmailThreadId>::new(),
        "every new thread gets a body after a transient full-resync failure"
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(5), core.stop())
            .await
            .is_ok(),
        "first core stops cleanly"
    );
}

#[tokio::test]
async fn reopening_mid_first_sync_downloads_remaining_bodies() {
    let base_ms = 1_762_000_000_000_i64;
    let restart_dir = tempfile::tempdir().unwrap();
    let restart_fake = sm_fake_gmail::FakeGmail::start();
    let (_user2, account2) =
        seed_body_recovery_fixture(restart_dir.path(), &restart_fake, base_ms).unwrap();
    inject_once_503(&restart_fake, "drafts.list");
    let core2 = ChaosEnv::open_core(restart_dir.path(), &restart_fake, Millis(base_ms))
        .await
        .unwrap();
    assert!(
        core2.wait_idle(Duration::from_secs(10)).await,
        "mid-first-sync failure settles before restart"
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(5), core2.stop())
            .await
            .is_ok(),
        "mid-first-sync core stops cleanly"
    );
    let reopened = ChaosEnv::open_core(restart_dir.path(), &restart_fake, Millis(base_ms + 60_000))
        .await
        .unwrap();
    assert!(
        reopened.wait_idle(Duration::from_secs(10)).await,
        "reopened core finishes first sync"
    );
    assert_eq!(
        missing_body_threads(&reopened, account2).unwrap(),
        Vec::<supermonkey_lib::domain::GmailThreadId>::new(),
        "reopening after mid-first-sync quit still downloads bodies for all inbox and starred threads"
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(5), reopened.stop())
            .await
            .is_ok(),
        "reopened core stops cleanly"
    );
}
