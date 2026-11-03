//! The single writer thread (SPEC UI state → Ordering).
//!
//! Every SQLite write runs here, one job at a time. Each job is one
//! transaction. A job that writes bumps `rev` and stamps the rows it touched.
//! After each commit the writer sends one channel message with the commit's
//! `rev`, `ack`, and the patches for every open subscription the commit
//! affects, all built from the same snapshot before commit.
//!
//! UI actions and first loads run before any queued sync job.

use std::collections::HashSet;

use crossbeam_channel::{Receiver, Sender, select_biased, unbounded};
use rusqlite::{Connection, Transaction, params};

use crate::clock::{Busy, Tracked};
use crate::model::{
    AccountId, ChannelMessage, DraftId, MessageId, Millis, NumberedAction, OutboxId, SubId,
    ThreadId,
};
use crate::subs::Subs;

type SyncFn = Box<dyn FnOnce(&mut Ctx<'_>) -> anyhow::Result<()> + Send>;
/// Where channel messages go: the webview's channel.
type ChannelSink = Box<dyn Fn(ChannelMessage) + Send>;

enum Job {
    Actions(Vec<NumberedAction>),
    Sync(SyncFn),
    Channel(ChannelSink),
    SetOnline(bool),
    /// Stop after the UI jobs queued before it; acknowledged once the database is closed.
    Stop(tokio::sync::oneshot::Sender<()>),
}

/// Post-commit work for the engine, each busy until the engine has handled it.
pub type EffectSender = tokio::sync::mpsc::UnboundedSender<Tracked<Effect>>;
pub type EffectReceiver = tokio::sync::mpsc::UnboundedReceiver<Tracked<Effect>>;

/// Work for the async side after a commit.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Effect {
    /// The account has queued Gmail ops to run.
    Ops(AccountId),
    /// The account's outbox changed.
    Outbox(AccountId),
    /// A conversation opened whose bodies may be missing; fetch at priority 2.
    FetchThread {
        account: AccountId,
        thread: ThreadId,
    },
    /// Run a Gmail search or view listing for a subscription.
    GmailList {
        sub: SubId,
        account: AccountId,
        query: GmailListQuery,
        page_token: Option<String>,
    },
    /// Cancel queued fetches for a subscription that closed or changed.
    CancelSub(SubId),
    /// An account was removed; stop its sync and revoke its token.
    AccountRemoved { account: AccountId, email: String },
    /// A draft was opened in a composer and needs its Gmail parts.
    DraftParts { account: AccountId, draft: DraftId },
    /// A composer closed without edits while a Gmail change was skipped.
    ReadDraft { account: AccountId, draft: DraftId },
    /// The account on screen changed (for request ordering).
    OnScreen(AccountId),
    /// Re-render the Dock badge.
    Badge,
    /// A notification should be raised (computed by sync).
    Notify(crate::notify::Notification),
    /// History or a fetch named a label id this account does not know.
    RefreshLabels(AccountId),
    /// A snooze time was set or changed; re-plan the wake timer.
    Snoozes,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GmailListQuery {
    Search(String),
    Label {
        label_gmail_id: String,
        include_spam_trash: bool,
    },
}

/// What a commit touched; decides which subscriptions get patches.
#[derive(Default, Debug)]
pub struct Touched {
    pub threads: HashSet<ThreadId>,
    /// Accounts whose thread membership may have changed.
    pub lists: HashSet<AccountId>,
    pub drafts: HashSet<DraftId>,
    pub draft_lists: HashSet<AccountId>,
    pub summaries: HashSet<AccountId>,
    pub subs: HashSet<SubId>,
    pub app: bool,
}

impl Touched {
    pub fn is_empty(&self) -> bool {
        self.threads.is_empty()
            && self.lists.is_empty()
            && self.drafts.is_empty()
            && self.draft_lists.is_empty()
            && self.summaries.is_empty()
            && self.subs.is_empty()
            && !self.app
    }

    /// A thread's row, its account's lists and summary, and unread counts.
    fn thread(&mut self, account: AccountId, thread: ThreadId) {
        self.threads.insert(thread);
        self.lists.insert(account);
        self.summaries.insert(account);
        self.app = true;
    }
}

/// Runs once the job's transaction has committed (`Ok`) or failed to (`Err`).
pub type OnCommit = Box<dyn FnOnce(Result<(), &anyhow::Error>)>;

/// A job's view of the writer: the open transaction plus bookkeeping.
pub struct Ctx<'a> {
    pub tx: &'a Transaction<'a>,
    /// The `rev` this transaction stamps.
    pub rev: i64,
    pub now: Millis,
    pub touched: Touched,
    pub effects: Vec<Effect>,
    pub subs: &'a mut Subs,
    pub online: bool,
    /// Callbacks for after the commit; dropped if the job itself fails.
    pub on_commit: Vec<OnCommit>,
}

impl Ctx<'_> {
    pub fn touch_thread(&mut self, account: AccountId, thread: ThreadId) -> rusqlite::Result<()> {
        self.tx.execute(
            "UPDATE threads SET rev = ?1 WHERE id = ?2",
            params![self.rev, thread],
        )?;
        self.touched.thread(account, thread);
        Ok(())
    }

    /// A thread was deleted; lists must drop it.
    pub fn thread_gone(&mut self, account: AccountId, thread: ThreadId) {
        self.touched.thread(account, thread);
    }

    pub fn touch_message(&mut self, message: MessageId) -> rusqlite::Result<()> {
        self.tx.execute(
            "UPDATE messages SET rev = ?1 WHERE id = ?2",
            params![self.rev, message],
        )?;
        Ok(())
    }

    pub fn touch_draft(&mut self, account: AccountId, draft: &str) -> rusqlite::Result<()> {
        self.tx.execute(
            "UPDATE drafts SET rev = ?1 WHERE id = ?2",
            params![self.rev, draft],
        )?;
        self.touched.drafts.insert(draft.to_string());
        self.touched.draft_lists.insert(account);
        self.touched.summaries.insert(account);
        Ok(())
    }

    pub fn touch_outbox(&mut self, account: AccountId, outbox: OutboxId) -> rusqlite::Result<()> {
        self.tx.execute(
            "UPDATE outbox SET rev = ?1 WHERE id = ?2",
            params![self.rev, outbox],
        )?;
        self.touched.summaries.insert(account);
        self.touched.draft_lists.insert(account);
        Ok(())
    }

    pub fn touch_summary(&mut self, account: AccountId) {
        self.touched.summaries.insert(account);
    }

    pub fn touch_app(&mut self) {
        self.touched.app = true;
    }

    pub fn effect(&mut self, e: Effect) {
        if !self.effects.contains(&e) {
            self.effects.push(e);
        }
    }
}

/// A queued job, busy until the writer has run it and sent its effects.
type Queued = (Job, Busy);

#[derive(Clone)]
pub struct Writer {
    ui: Sender<Queued>,
    sync: Sender<Queued>,
}

impl Writer {
    /// Starts the writer thread. `effects` receives post-commit work.
    pub fn start(db_path: &std::path::Path, effects: EffectSender) -> anyhow::Result<Writer> {
        let conn = crate::db::open_writer(db_path)?;
        let (ui_tx, ui_rx) = unbounded();
        let (sync_tx, sync_rx) = unbounded();
        std::thread::Builder::new()
            .name("writer".into())
            .spawn(move || {
                let mut state = WriterState::new(conn, effects);
                let stopped = state.run(&ui_rx, &sync_rx);
                // Close the database and drop unrun jobs before saying so.
                drop(state);
                drop((ui_rx, sync_rx));
                if let Some(ack) = stopped {
                    let _ = ack.send(());
                }
            })?;
        Ok(Writer {
            ui: ui_tx,
            sync: sync_tx,
        })
    }

    fn queue_ui(&self, job: Job) {
        let _ = self.ui.send((job, Busy::new("writer")));
    }

    /// Queues UI actions. Returns as soon as they are queued.
    pub fn actions(&self, actions: Vec<NumberedAction>) {
        self.queue_ui(Job::Actions(actions));
    }

    /// Queues a sync job (runs after pending UI work).
    pub fn sync(&self, f: impl FnOnce(&mut Ctx<'_>) -> anyhow::Result<()> + Send + 'static) {
        let _ = self
            .sync
            .send((Job::Sync(Box::new(f)), Busy::new("writer")));
    }

    /// Stops the thread after the UI jobs queued so far, as quitting the app
    /// would; queued sync jobs are dropped. Returns once the database is closed.
    /// The E2E server restarts the core in-process this way.
    pub async fn stop(&self) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.queue_ui(Job::Stop(tx));
        if rx.await.is_err() {
            log::debug!("writer was already stopped");
        }
    }

    /// Runs a job and waits for its result.
    pub async fn call<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Ctx<'_>) -> anyhow::Result<T> + Send + 'static,
    ) -> anyhow::Result<T> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.sync(move |ctx| run_and_reply(ctx, f, move |r| _ = tx.send(r)));
        rx.await
            .map_err(|e| anyhow::anyhow!("writer stopped: {e}"))?
    }

    /// Blocking variant of `call` for non-async code (tests, setup).
    pub fn call_blocking<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Ctx<'_>) -> anyhow::Result<T> + Send + 'static,
    ) -> anyhow::Result<T> {
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.sync(move |ctx| run_and_reply(ctx, f, move |r| _ = tx.send(r)));
        rx.recv()
            .map_err(|e| anyhow::anyhow!("writer stopped: {e}"))?
    }

    /// Sets where channel messages go (the webview's channel).
    pub fn set_channel(&self, send: impl Fn(ChannelMessage) + Send + 'static) {
        self.queue_ui(Job::Channel(Box::new(send)));
    }

    pub fn set_online(&self, online: bool) {
        self.queue_ui(Job::SetOnline(online));
    }
}

/// Runs `f` and hands its result to the waiting caller. A failed `f` also
/// fails the job, so its transaction rolls back.
/// Replies with the job's result once its transaction has committed, so the
/// caller's next read (from any connection) sees what the job wrote.
fn run_and_reply<T: 'static>(
    ctx: &mut Ctx<'_>,
    f: impl FnOnce(&mut Ctx<'_>) -> anyhow::Result<T>,
    reply: impl FnOnce(anyhow::Result<T>) + 'static,
) -> anyhow::Result<()> {
    match f(ctx) {
        Ok(value) => {
            ctx.on_commit.push(Box::new(move |committed| {
                reply(match committed {
                    Ok(()) => Ok(value),
                    Err(e) => Err(anyhow::anyhow!("commit failed: {e:#}")),
                });
            }));
            Ok(())
        }
        // The transaction rolls back; nothing to wait for.
        Err(e) => {
            reply(Err(anyhow::anyhow!("{e:#}")));
            anyhow::bail!("job failed")
        }
    }
}

struct WriterState {
    conn: Connection,
    rev: i64,
    ack: i64,
    subs: Subs,
    channel: Option<ChannelSink>,
    effects: EffectSender,
    online: bool,
}

impl WriterState {
    fn new(conn: Connection, effects: EffectSender) -> WriterState {
        let rev = conn
            .query_row("SELECT value FROM meta WHERE key = 'rev'", [], |r| r.get(0))
            .unwrap_or(0);
        let ack = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'last_action'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        WriterState {
            conn,
            rev,
            ack,
            subs: Subs::default(),
            channel: None,
            effects,
            online: true,
        }
    }

    /// Runs jobs until the queues close or a stop arrives, which it returns.
    fn run(
        &mut self,
        ui: &Receiver<Queued>,
        sync: &Receiver<Queued>,
    ) -> Option<tokio::sync::oneshot::Sender<()>> {
        loop {
            let job = select_biased! {
                recv(ui) -> j => j,
                recv(sync) -> j => j,
            };
            // The job's activity ends after it ran and sent its effects.
            let Ok((job, _busy)) = job else { return None };
            match job {
                Job::Stop(ack) => return Some(ack),
                Job::Actions(actions) => {
                    for a in actions {
                        self.apply_action(a);
                    }
                }
                Job::Sync(f) => self.apply_sync(f),
                Job::Channel(c) => self.channel = Some(c),
                Job::SetOnline(online) => {
                    if self.online != online {
                        self.online = online;
                        self.apply_sync(|ctx| {
                            ctx.touch_app();
                            Ok(())
                        });
                    }
                }
            }
        }
    }

    fn apply_action(&mut self, a: NumberedAction) {
        // Numbers never go back; a reload asks for the last one first.
        if a.n <= self.ack {
            log::warn!("dropping action {} at or below ack {}", a.n, self.ack);
            return;
        }
        let n = a.n;
        let result = self.transact(Some(n), |ctx| crate::actions::apply(ctx, n, a.action));
        if let Err(e) = result {
            log::warn!("action {n} rejected: {e:#}");
            // The rejected action still moves ack, so the mirror falls back to Rust's state.
            if let Err(e) = self.transact(Some(n), |_| Ok(())) {
                log::error!("ack for rejected action {n} failed: {e:#}");
            }
        }
    }

    fn apply_sync(&mut self, f: impl FnOnce(&mut Ctx<'_>) -> anyhow::Result<()>) {
        if let Err(e) = self.transact(None, f) {
            log::warn!("sync job failed: {e:#}");
        }
    }

    /// Runs one transaction, then builds and sends the channel message.
    fn transact(
        &mut self,
        action: Option<i64>,
        f: impl FnOnce(&mut Ctx<'_>) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        let tx = self.conn.transaction()?;
        let rev = self.rev + 1;
        let mut ctx = Ctx {
            tx: &tx,
            rev,
            now: crate::clock::now_ms(),
            touched: Touched::default(),
            effects: Vec::new(),
            subs: &mut self.subs,
            online: self.online,
            on_commit: Vec::new(),
        };
        f(&mut ctx)?;
        let Ctx {
            touched,
            effects,
            on_commit,
            ..
        } = ctx;
        let wrote = !touched.is_empty();
        let touched_app = touched.app;
        let ack = action.unwrap_or(self.ack).max(self.ack);
        let subs = &mut self.subs;
        let online = self.online;
        let committed = (move || -> anyhow::Result<_> {
            if let Some(n) = action {
                tx.execute("UPDATE meta SET value = ?1 WHERE key = 'last_action'", [n])?;
            }
            if wrote {
                tx.execute("UPDATE meta SET value = ?1 WHERE key = 'rev'", [rev])?;
            }
            let mut updates = subs.updates(&tx, &touched, online)?;
            updates.extend(subs.take_first_loads(&tx, online)?);
            tx.commit()?;
            Ok(updates)
        })();
        for done in on_commit {
            done(committed.as_ref().map(|_| ()));
        }
        let updates = committed?;
        if wrote {
            self.rev = rev;
        }
        let ack_moved = ack != self.ack;
        self.ack = ack;
        if (!updates.is_empty() || ack_moved)
            && let Some(send) = &self.channel
        {
            send(ChannelMessage {
                rev: self.rev,
                ack,
                updates,
            });
        }
        // Unread counts may have changed: refresh the Dock badge.
        if touched_app {
            let _ = self.effects.send(Tracked::new("effect", Effect::Badge));
        }
        for e in effects {
            let _ = self.effects.send(Tracked::new("effect", e));
        }
        Ok(())
    }
}
