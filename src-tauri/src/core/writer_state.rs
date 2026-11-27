use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::domain::{
    AccountId, ActionNo, DraftId, GmailDraftId, GmailMessageId, HistoryId, LabelId, Rev, SubId,
    ThreadChange, ThreadId,
};
use crate::ipc::{ChannelMessage, RejectReason, Rejection, SubKey};
use crate::store::{Commit, Db};

use super::events::{Effect, EffectBus};
use super::runtime::Clock;
use super::subs::{ListingState, Registry};

const MAX_LANDED_OPS: usize = 128;

#[derive(Clone)]
pub struct ChannelSink(Arc<dyn Fn(ChannelMessage) + Send + Sync>);

impl ChannelSink {
    #[must_use]
    pub fn new(f: impl Fn(ChannelMessage) + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    pub fn send(&self, msg: ChannelMessage) {
        (self.0)(msg);
    }
}

impl std::fmt::Debug for ChannelSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ChannelSink(..)")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModifyOpPayload {
    pub(crate) thread: ThreadId,
    pub(crate) gmail_msg_ids: Vec<GmailMessageId>,
    pub(crate) add: Vec<LabelId>,
    pub(crate) remove: Vec<LabelId>,
    pub(crate) revert: ThreadChange,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateLabelOpPayload {
    pub(crate) local_id: LabelId,
    pub(crate) name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DraftOpPayload {
    pub(crate) draft_id: DraftId,
    pub(crate) gmail_draft_id: Option<GmailDraftId>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DraftOpPayloadRef<'a> {
    pub(crate) draft_id: &'a DraftId,
    pub(crate) gmail_draft_id: Option<&'a GmailDraftId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TrashSentOpPayload {
    pub(crate) gmail_msg_id: GmailMessageId,
}

#[derive(Debug, Clone)]
pub(crate) struct LandedOp {
    pub(crate) gmail_msg_ids: Vec<GmailMessageId>,
    pub(crate) add: Vec<LabelId>,
    pub(crate) remove: Vec<LabelId>,
    pub(crate) at_rev: Rev,
    pub(crate) min_history_id: Option<HistoryId>,
}

#[derive(Debug, Default)]
pub(crate) struct Freshness {
    labels_gen: HashMap<AccountId, u64>,
    send_as_gen: HashMap<AccountId, u64>,
    landed: HashMap<AccountId, VecDeque<LandedOp>>,
}

impl Freshness {
    #[must_use]
    pub(crate) fn labels_gen(&self, account: AccountId) -> u64 {
        self.labels_gen.get(&account).copied().unwrap_or(0)
    }

    pub(crate) fn bump_labels(&mut self, account: AccountId) -> u64 {
        let slot = self.labels_gen.entry(account).or_insert(0);
        *slot = slot.saturating_add(1);
        *slot
    }

    #[must_use]
    pub(crate) fn send_as_gen(&self, account: AccountId) -> u64 {
        self.send_as_gen.get(&account).copied().unwrap_or(0)
    }

    pub(crate) fn bump_send_as(&mut self, account: AccountId) -> u64 {
        let slot = self.send_as_gen.entry(account).or_insert(0);
        *slot = slot.saturating_add(1);
        *slot
    }

    pub(crate) fn record_landed(&mut self, account: AccountId, op: LandedOp) {
        let list = self.landed.entry(account).or_default();
        list.push_back(op);
        while list.len() > MAX_LANDED_OPS {
            list.pop_front();
        }
    }

    #[must_use]
    pub(crate) fn landed_since(&self, account: AccountId, since_rev: Rev) -> Vec<LandedOp> {
        self.landed
            .get(&account)
            .map(|list| {
                list.iter()
                    .filter(|op| op.at_rev > since_rev)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) fn clear_account(&mut self, account: AccountId) {
        self.labels_gen.remove(&account);
        self.send_as_gen.remove(&account);
        self.landed.remove(&account);
    }
}

#[derive(Debug, Clone)]
pub(crate) enum SubOp {
    Reset,
    Subscribe(SubId, SubKey),
    Unsubscribe(SubId),
    DropComposer(DraftId),
    SetListing(SubId, ListingState),
    StopSearching(AccountId),
}

pub struct WriterState {
    pub(crate) registry: Registry,
    pub(crate) freshness: Freshness,
    pub(crate) ack: ActionNo,
    pub(crate) last_rev: Rev,
    pub(crate) sent_rev: Rev,
    pub(crate) online: bool,
    pub(crate) clock: Clock,
    pub(crate) bus: EffectBus,
    pub(crate) channel: Arc<Mutex<Option<ChannelSink>>>,
    staged_effects: Vec<Effect>,
    staged_sub_ops: Vec<SubOp>,
    staged_force_subs: HashSet<SubId>,
    pub(crate) staged_rejections: Vec<Rejection>,
    pub(crate) staged_ack: Option<ActionNo>,
    pub(crate) staged_batch_ns: Vec<ActionNo>,
    staged_online: Option<bool>,
}

impl std::fmt::Debug for WriterState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WriterState")
            .field("ack", &self.ack)
            .field("last_rev", &self.last_rev)
            .field("online", &self.online)
            .finish_non_exhaustive()
    }
}

impl WriterState {
    #[expect(
        clippy::too_many_arguments,
        reason = "writer state owns initial cursors, network flag, clock, effect bus, and channel sink"
    )]
    #[must_use]
    pub(crate) fn new(
        ack: ActionNo,
        last_rev: Rev,
        online: bool,
        clock: Clock,
        bus: EffectBus,
        channel: Arc<Mutex<Option<ChannelSink>>>,
    ) -> Self {
        Self {
            registry: Registry::new(),
            freshness: Freshness::default(),
            ack,
            last_rev,
            sent_rev: Rev(0),
            online,
            clock,
            bus,
            channel,
            staged_effects: Vec::new(),
            staged_sub_ops: Vec::new(),
            staged_force_subs: HashSet::new(),
            staged_rejections: Vec::new(),
            staged_ack: None,
            staged_batch_ns: Vec::new(),
            staged_online: None,
        }
    }

    pub(crate) fn emit(&mut self, effect: Effect) {
        self.staged_effects.push(effect);
    }

    pub(crate) fn stage_sub(&mut self, op: SubOp) {
        self.staged_sub_ops.push(op);
    }

    pub(crate) const fn stage_online(&mut self, online: bool) {
        self.staged_online = Some(online);
    }

    #[must_use]
    pub(crate) const fn effects_mark(&self) -> (usize, usize) {
        (self.staged_effects.len(), self.staged_sub_ops.len())
    }

    pub(crate) fn truncate_staged(&mut self, mark: (usize, usize)) {
        self.staged_effects.truncate(mark.0);
        self.staged_sub_ops.truncate(mark.1);
    }

    fn apply_sub_ops(&mut self, sub_ops: Vec<SubOp>, force_subs: &mut HashSet<SubId>) {
        for op in sub_ops {
            match op {
                SubOp::Reset => self.registry.clear(),
                SubOp::Subscribe(id, key) => {
                    self.registry.subscribe(id.clone(), key);
                    force_subs.insert(id);
                }
                SubOp::Unsubscribe(id) => {
                    self.registry.unsubscribe(&id);
                }
                SubOp::DropComposer(draft) => {
                    let drop_ids = self.registry.all_composer_subs(&draft);
                    for id in drop_ids {
                        self.registry.unsubscribe(&id);
                    }
                }
                SubOp::SetListing(id, listing) => {
                    if self.registry.set_listing(&id, listing) {
                        force_subs.insert(id);
                    }
                }
                SubOp::StopSearching(account) => {
                    for id in self.registry.stop_searching_for_account(account) {
                        force_subs.insert(id);
                    }
                }
            }
        }
    }

    pub(crate) fn on_commit(&mut self, db: Db<'_>, commit: &Commit) {
        self.last_rev = commit.rev;
        if let Some(ack) = self.staged_ack.take() {
            self.ack = self.ack.max(ack);
        }
        let online_changed = self.staged_online.take().is_some_and(|next| {
            let changed = self.online != next;
            self.online = next;
            changed
        });
        let sub_ops = std::mem::take(&mut self.staged_sub_ops);
        let mut force_subs = std::mem::take(&mut self.staged_force_subs);
        self.apply_sub_ops(sub_ops, &mut force_subs);
        if online_changed {
            for id in self.registry.app_and_search_subs() {
                force_subs.insert(id);
            }
        }

        let rejected = std::mem::take(&mut self.staged_rejections);
        let had_batch = !self.staged_batch_ns.is_empty();
        self.staged_batch_ns.clear();
        let updates = self
            .registry
            .evaluate_commit(
                db,
                &commit.changes,
                commit.rev,
                self.online,
                &self.clock,
                &force_subs,
            )
            .unwrap_or_else(|err| {
                super::log_err::<(), _>("project subscription updates", Err(err));
                Vec::new()
            });

        let effects = std::mem::take(&mut self.staged_effects);
        self.bus.emit_all(effects);

        if had_batch || !rejected.is_empty() || !updates.is_empty() {
            self.sent_rev = self.sent_rev.max(commit.rev);
            let msg = ChannelMessage {
                rev: commit.rev,
                ack: self.ack,
                rejected,
                updates,
            };
            self.send_channel(msg);
        }
    }

    pub(crate) fn on_rollback(&mut self) {
        self.staged_effects.clear();
        self.staged_sub_ops.clear();
        self.staged_force_subs.clear();
        self.staged_rejections.clear();
        self.staged_ack = None;
        self.staged_online = None;
        let failed = std::mem::take(&mut self.staged_batch_ns);
        if !failed.is_empty() {
            let rejected = failed
                .into_iter()
                .map(|n| Rejection {
                    n,
                    message: "Something went wrong inside Supermonkey".to_owned(),
                    reason: RejectReason::Other,
                })
                .collect();
            self.send_channel(ChannelMessage {
                rev: self.last_rev,
                ack: self.ack,
                rejected,
                updates: Vec::new(),
            });
        }
    }

    fn send_channel(&self, msg: ChannelMessage) {
        let sink = self.channel.lock().clone();
        if let Some(sink) = sink {
            sink.send(msg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::actions::apply_batch;
    use crate::core::runtime::{Activity, Busy};
    use crate::domain::Millis;
    use crate::ipc::{Action, NumberedAction, SubUpdate, Submitted};
    use crate::store::{Priority as StorePriority, StoreError, Writer};

    struct TestWriter {
        _dir: tempfile::TempDir,
        writer: Writer<WriterState>,
        effects_rx: tokio::sync::mpsc::UnboundedReceiver<(Effect, Busy)>,
        msgs: Arc<Mutex<Vec<ChannelMessage>>>,
    }

    fn test_writer() -> TestWriter {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("mail.db");
        let (bus, effects_rx) = EffectBus::new(Activity::new());
        let msgs: Arc<Mutex<Vec<ChannelMessage>>> = Arc::new(Mutex::new(Vec::new()));
        let sink_msgs = Arc::clone(&msgs);
        let channel = Arc::new(Mutex::new(Some(ChannelSink::new(move |msg| {
            sink_msgs.lock().push(msg);
        }))));
        let state = WriterState::new(
            ActionNo(0),
            Rev(0),
            true,
            Clock::manual(Millis(1_000)),
            bus,
            channel,
        );
        let writer = Writer::start(
            &db_path,
            state,
            WriterState::on_commit,
            WriterState::on_rollback,
        )
        .unwrap();
        TestWriter {
            _dir: dir,
            writer,
            effects_rx,
            msgs,
        }
    }

    fn sub_app(n: i64) -> Vec<Submitted> {
        vec![Submitted::from(NumberedAction {
            n: ActionNo(n),
            action: Action::Subscribe {
                sub: SubId("s1".to_owned()),
                key: SubKey::App,
            },
        })]
    }

    #[test]
    fn failed_batch_commit_rejects_actions_drops_effects_and_keeps_first_load() {
        let TestWriter {
            _dir,
            writer,
            mut effects_rx,
            msgs,
        } = test_writer();

        let err = writer
            .call(StorePriority::Ui, |tx, st| {
                apply_batch(tx, st, sub_app(1))?;
                st.emit(Effect::RefreshBadge);
                tx.fail_commit();
                Ok::<_, StoreError>(())
            })
            .wait();
        assert!(err.is_err(), "fail_commit aborts transaction");
        assert!(
            effects_rx.try_recv().is_err(),
            "rollback drops staged effects"
        );

        let sent = msgs.lock().clone();
        assert_eq!(sent.len(), 1, "rollback emits one channel message");
        assert_eq!(sent[0].ack, ActionNo(0), "ack does not advance on rollback");
        assert_eq!(sent[0].rejected.len(), 1, "failed batch action is rejected");
        assert_eq!(
            sent[0].rejected[0].n,
            ActionNo(1),
            "rejection names failed action"
        );

        writer
            .call(StorePriority::Ui, |tx, st| apply_batch(tx, st, sub_app(2)))
            .wait()
            .unwrap();
        let last = msgs.lock().last().cloned().unwrap();
        assert_eq!(last.ack, ActionNo(2), "subsequent batch advances ack");
        assert!(
            matches!(
                last.updates.as_slice(),
                [SubUpdate::App { first: true, .. }]
            ),
            "retry subscribe still delivers first snapshot"
        );
        writer.stop().wait().unwrap();
    }

    #[test]
    fn send_channel_releases_lock_before_invoking_sink() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("reentrant.db");
        drop(crate::store::open_writer(&db_path).unwrap());
        let activity = Activity::new();
        let (bus, _rx) = EffectBus::new(activity);
        let channel: Arc<Mutex<Option<ChannelSink>>> = Arc::new(Mutex::new(None));
        let sink_channel = Arc::clone(&channel);
        let acquired = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let acquired_c = Arc::clone(&acquired);
        *channel.lock() = Some(ChannelSink::new(move |_msg| {
            let unlocked = sink_channel.try_lock().is_some();
            acquired_c.store(unlocked, std::sync::atomic::Ordering::SeqCst);
        }));
        let state = WriterState::new(
            ActionNo(0),
            Rev(0),
            true,
            Clock::manual(Millis(1_000)),
            bus,
            Arc::clone(&channel),
        );
        let writer = Writer::start(
            &db_path,
            state,
            WriterState::on_commit,
            WriterState::on_rollback,
        )
        .unwrap();
        writer
            .call(StorePriority::Ui, |tx, st| apply_batch(tx, st, sub_app(1)))
            .wait()
            .unwrap();
        assert!(
            acquired.load(std::sync::atomic::Ordering::SeqCst),
            "channel lock must be released before invoking external ChannelSink callback"
        );
        writer.stop().wait().unwrap();
    }
}
