use std::any::Any;
use std::collections::VecDeque;
use std::fmt;
use std::num::NonZeroUsize;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use parking_lot::{Condvar, Mutex};
use rusqlite::{Connection, TransactionBehavior};
use tokio::sync::oneshot;

use crate::domain::Rev;

use super::{Commit, Db, Finished, StoreError, WriteTx, open_reader, open_writer};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    Ui,
    Sync,
    Background,
}

pub struct Pending<T, E> {
    rx: oneshot::Receiver<Result<T, E>>,
}

impl<T, E> fmt::Debug for Pending<T, E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pending").finish_non_exhaustive()
    }
}

impl<T, E: From<StoreError>> Pending<T, E> {
    pub fn wait(self) -> Result<T, E> {
        self.rx
            .blocking_recv()
            .unwrap_or_else(|_closed| Err(StoreError::WriterStopped.into()))
    }
}

impl<T, E: From<StoreError>> Future for Pending<T, E> {
    type Output = Result<T, E>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.rx)
            .poll(cx)
            .map(|res| res.unwrap_or_else(|_closed| Err(StoreError::WriterStopped.into())))
    }
}

type JobFn<S> = dyn FnOnce(&mut Runner<S>) + Send;
type CommitFn<S> = dyn FnMut(&mut S, Db<'_>, &Commit) + Send;

enum Entry<S> {
    Job(Box<JobFn<S>>),
    Stop,
}

struct Queue<S> {
    ui: VecDeque<Entry<S>>,
    sync: VecDeque<Entry<S>>,
    background: VecDeque<Entry<S>>,
    stopping: bool,
    stopped: bool,
    closed: bool,
    stop_waiters: Vec<oneshot::Sender<Result<(), StoreError>>>,
}

impl<S> Queue<S> {
    fn pop(&mut self) -> Option<Entry<S>> {
        self.ui
            .pop_front()
            .or_else(|| self.sync.pop_front())
            .or_else(|| self.background.pop_front())
    }
}

struct Shared<S> {
    queue: Mutex<Queue<S>>,
    ready: Condvar,
}

impl<S> Shared<S> {
    fn push(&self, priority: Priority, entry: Entry<S>) -> bool {
        let mut queue = self.queue.lock();
        if queue.closed || queue.stopping {
            return false;
        }
        match priority {
            Priority::Ui => queue.ui.push_back(entry),
            Priority::Sync => queue.sync.push_back(entry),
            Priority::Background => queue.background.push_back(entry),
        }
        drop(queue);
        self.ready.notify_one();
        true
    }

    fn request_stop(&self) -> oneshot::Receiver<Result<(), StoreError>> {
        let (tx, rx) = oneshot::channel();
        let mut queue = self.queue.lock();
        if queue.stopped {
            drop(tx.send(Ok(())));
            return rx;
        }
        queue.stop_waiters.push(tx);
        if !queue.stopping {
            queue.stopping = true;
            queue.ui.push_back(Entry::Stop);
        }
        drop(queue);
        self.ready.notify_one();
        rx
    }

    fn close(&self) {
        self.queue.lock().closed = true;
        self.ready.notify_one();
    }
}

struct Handle<S> {
    shared: Arc<Shared<S>>,
}

impl<S> Drop for Handle<S> {
    fn drop(&mut self) {
        self.shared.close();
    }
}

pub struct Writer<S> {
    handle: Arc<Handle<S>>,
}

impl<S> fmt::Debug for Writer<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Writer").finish_non_exhaustive()
    }
}

impl<S> Clone for Writer<S> {
    fn clone(&self) -> Self {
        Self {
            handle: Arc::clone(&self.handle),
        }
    }
}

impl<S: Send + 'static> Writer<S> {
    pub fn start(
        path: &Path,
        state: S,
        on_commit: impl FnMut(&mut S, Db<'_>, &Commit) + Send + 'static,
        on_rollback: impl FnMut(&mut S) + Send + 'static,
    ) -> Result<Self, StoreError> {
        let conn = open_writer(path)?;
        let rev = Db(&conn).rev()?;
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue {
                ui: VecDeque::new(),
                sync: VecDeque::new(),
                background: VecDeque::new(),
                stopping: false,
                stopped: false,
                closed: false,
                stop_waiters: Vec::new(),
            }),
            ready: Condvar::new(),
        });
        let runner = Runner {
            conn,
            rev,
            state,
            on_commit: Box::new(on_commit),
            on_rollback: Box::new(on_rollback),
        };
        let thread_shared = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("sm-writer".to_owned())
            .spawn(move || run(&thread_shared, runner))
            .map_err(|source| StoreError::Io {
                action: "start the writer thread for",
                path: path.to_path_buf(),
                source,
            })?;
        Ok(Self {
            handle: Arc::new(Handle { shared }),
        })
    }

    pub fn call<T, E, F>(&self, priority: Priority, job: F) -> Pending<T, E>
    where
        T: Send + 'static,
        E: From<StoreError> + Send + 'static,
        F: FnOnce(&mut WriteTx<'_>, &mut S) -> Result<T, E> + Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        let entry = Entry::Job(Box::new(move |runner: &mut Runner<S>| {
            drop(tx.send(runner.transact(job)));
        }));
        let _ = self.handle.shared.push(priority, entry);
        Pending { rx }
    }

    #[must_use]
    pub fn stop(&self) -> Pending<(), StoreError> {
        Pending {
            rx: self.handle.shared.request_stop(),
        }
    }
}

struct Runner<S> {
    conn: Connection,
    rev: Rev,
    state: S,
    on_commit: Box<CommitFn<S>>,
    on_rollback: Box<dyn FnMut(&mut S) + Send>,
}

impl<S> Runner<S> {
    fn transact<T, E: From<StoreError>>(
        &mut self,
        job: impl FnOnce(&mut WriteTx<'_>, &mut S) -> Result<T, E>,
    ) -> Result<T, E> {
        let Self {
            conn,
            rev,
            state,
            on_commit,
            on_rollback,
        } = self;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StoreError::sql("begin a write transaction"))?;
        let next = Rev(rev.0.saturating_add(1));
        let mut write = WriteTx::new(&tx, next);
        let value = match run_job(&mut write, state, job) {
            Ok(value) => value,
            Err(err) => {
                drop(write);
                drop(tx);
                rolled_back(on_rollback.as_mut(), state);
                return Err(err);
            }
        };
        let finished = write.finish();
        if let Err(err) = commit_tx(tx, next, &finished) {
            rolled_back(on_rollback.as_mut(), state);
            return Err(err.into());
        }
        if finished.wrote {
            *rev = next;
        }
        let commit = Commit {
            rev: *rev,
            changes: finished.changes,
        };
        committed(on_commit.as_mut(), state, conn, &commit);
        Ok(value)
    }
}

fn run_job<S, T, E: From<StoreError>>(
    write: &mut WriteTx<'_>,
    state: &mut S,
    job: impl FnOnce(&mut WriteTx<'_>, &mut S) -> Result<T, E>,
) -> Result<T, E> {
    match catch_unwind(AssertUnwindSafe(|| job(write, state))) {
        Ok(result) => result,
        Err(panic) => {
            let message = panic_message(panic.as_ref());
            tracing::error!(%message, "writer job panicked; rolled back");
            Err(StoreError::JobPanicked(message).into())
        }
    }
}

fn commit_tx(
    tx: rusqlite::Transaction<'_>,
    next: Rev,
    finished: &Finished,
) -> Result<(), StoreError> {
    let res = (|| {
        if finished.wrote {
            Db(&tx).execute(
                "UPDATE meta SET rev = ?1 WHERE id = 1",
                [next],
                "update meta rev",
            )?;
        }
        #[cfg(any(test, feature = "fake"))]
        if finished.fail_commit {
            Db(&tx).execute(
                "UPDATE meta SET id = 2 WHERE id = 1",
                [],
                "force commit failure",
            )?;
        }
        tx.commit()
            .map_err(StoreError::sql("commit a write transaction"))
    })();
    if let Err(e) = &res {
        tracing::warn!(error = %e, "write transaction failed to commit");
    }
    res
}

fn committed<S>(
    on_commit: &mut dyn FnMut(&mut S, Db<'_>, &Commit),
    state: &mut S,
    conn: &Connection,
    commit: &Commit,
) {
    if let Err(panic) = catch_unwind(AssertUnwindSafe(|| on_commit(state, Db(conn), commit))) {
        let message = panic_message(panic.as_ref());
        tracing::error!(%message, rev = commit.rev.0, "on_commit panicked");
    }
}

fn rolled_back<S>(on_rollback: &mut dyn FnMut(&mut S), state: &mut S) {
    if let Err(panic) = catch_unwind(AssertUnwindSafe(|| on_rollback(state))) {
        let message = panic_message(panic.as_ref());
        tracing::error!(%message, "on_rollback panicked");
    }
}

fn panic_message(panic: &(dyn Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic with no message".to_owned())
}

fn run<S>(shared: &Shared<S>, mut runner: Runner<S>) {
    loop {
        let entry = {
            let mut queue = shared.queue.lock();
            loop {
                if let Some(entry) = queue.pop() {
                    break entry;
                }
                if queue.closed {
                    queue.stopped = true;
                    let waiters = std::mem::take(&mut queue.stop_waiters);
                    drop(queue);
                    drop(runner);
                    for tx in waiters {
                        drop(tx.send(Ok(())));
                    }
                    return;
                }
                shared.ready.wait(&mut queue);
            }
        };
        match entry {
            Entry::Job(job) => job(&mut runner),
            Entry::Stop => {
                let (dropped_ui, dropped_sync, dropped_bg, waiters) = {
                    let mut queue = shared.queue.lock();
                    queue.closed = true;
                    queue.stopped = true;
                    (
                        std::mem::take(&mut queue.ui),
                        std::mem::take(&mut queue.sync),
                        std::mem::take(&mut queue.background),
                        std::mem::take(&mut queue.stop_waiters),
                    )
                };
                drop(dropped_ui);
                drop(dropped_sync);
                drop(dropped_bg);
                drop(runner);
                for tx in waiters {
                    drop(tx.send(Ok(())));
                }
                return;
            }
        }
    }
}

pub struct ReadPool {
    idle: Mutex<Vec<Connection>>,
    ready: Condvar,
}

impl fmt::Debug for ReadPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReadPool")
            .field("idle", &self.idle.lock().len())
            .finish_non_exhaustive()
    }
}

impl ReadPool {
    pub fn open(path: &Path, size: NonZeroUsize) -> Result<Self, StoreError> {
        let mut conns = Vec::with_capacity(size.get());
        for _ in 0..size.get() {
            conns.push(open_reader(path)?);
        }
        Ok(Self {
            idle: Mutex::new(conns),
            ready: Condvar::new(),
        })
    }

    pub fn with<T, E>(&self, read: impl FnOnce(Db<'_>) -> Result<T, E>) -> Result<T, E>
    where
        E: From<StoreError>,
    {
        let mut lease = self.checkout();
        let Some(conn) = lease.conn.as_mut() else {
            return Err(StoreError::Invalid("a read connection went missing").into());
        };
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(StoreError::sql("begin read transaction"))?;
        read(Db(&tx))
    }

    fn checkout(&self) -> Lease<'_> {
        let mut idle = self.idle.lock();
        loop {
            if let Some(conn) = idle.pop() {
                return Lease {
                    pool: self,
                    conn: Some(conn),
                };
            }
            self.ready.wait(&mut idle);
        }
    }
}

struct Lease<'p> {
    pool: &'p ReadPool,
    conn: Option<Connection>,
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            self.pool.idle.lock().push(conn);
            self.pool.ready.notify_one();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Barrier;

    use super::*;

    #[tokio::test]
    #[expect(
        clippy::too_many_lines,
        reason = "end-to-end actor unit test verifies priority order, commit, rollback, and stop"
    )]
    async fn writer_orders_priorities_and_rolls_back_failed_commit() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("mail.db");
        let rollbacks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let rb = Arc::clone(&rollbacks);
        let writer = Writer::start(
            &db_path,
            (),
            |(), _db, _commit| {},
            move |()| {
                rb.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            },
        )
        .unwrap();

        let gate = Arc::new(Barrier::new(2));
        let g = Arc::clone(&gate);
        let blocker = writer.call(Priority::Ui, move |_tx, ()| {
            g.wait();
            Ok::<_, StoreError>(())
        });

        let order = Arc::new(Mutex::new(Vec::new()));
        let o1 = Arc::clone(&order);
        let bg = writer.call(Priority::Background, move |_tx, ()| {
            o1.lock().push("bg");
            Ok::<_, StoreError>(())
        });
        let o2 = Arc::clone(&order);
        let sync = writer.call(Priority::Sync, move |_tx, ()| {
            o2.lock().push("sync");
            Ok::<_, StoreError>(())
        });
        let o3 = Arc::clone(&order);
        let ui = writer.call(Priority::Ui, move |_tx, ()| {
            o3.lock().push("ui");
            Ok::<_, StoreError>(())
        });

        gate.wait();
        blocker.await.unwrap();
        ui.await.unwrap();
        sync.await.unwrap();
        bg.await.unwrap();
        assert_eq!(*order.lock(), vec!["ui", "sync", "bg"]);

        let failed: Result<(), StoreError> = writer
            .call(Priority::Ui, |tx, ()| {
                tx.create_account("fail@example.com", "Fail")?;
                tx.fail_commit();
                Ok(())
            })
            .await;
        assert!(failed.is_err());
        assert_eq!(rollbacks.load(std::sync::atomic::Ordering::SeqCst), 1);

        let pool = ReadPool::open(&db_path, NonZeroUsize::new(1).unwrap()).unwrap();
        #[expect(
            clippy::redundant_closure_for_method_calls,
            reason = "Db carries a lifetime so a closure is needed for ReadPool::with HRTB"
        )]
        let accounts = pool.with(|db| db.accounts()).unwrap();
        assert_eq!(accounts, []);
        #[expect(
            clippy::redundant_closure_for_method_calls,
            reason = "Db carries a lifetime so a closure is needed for ReadPool::with HRTB"
        )]
        let rev = pool.with(|db| db.rev()).unwrap();
        assert_eq!(rev, Rev(0));
        writer.stop().await.unwrap();
    }
}
