//! The writer actor (ARCHITECTURE.md, sections 4 and 6; SPEC UI state,
//! Ordering): one OS thread owns the only write connection and runs jobs one
//! at a time, highest priority first and in arrival order within a priority.
//! Each job is one transaction.
//!
//! The queue is a mutex over three lanes plus a condvar, not a channel: a
//! channel can't take the highest lane first, and a bounded one would block
//! an async caller's thread when full. It is unbounded: a caller that awaits
//! its reply has one job queued at a time, and the core's fire-and-forget
//! writes are a fixed few per Gmail answer.

use std::any::Any;
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::Arc;
use std::thread::ThreadId;

use parking_lot::{Condvar, Mutex};
use rusqlite::{Connection, TransactionBehavior};
use ub_domain::Rev;

use crate::meta;
use crate::reply::{self, Pending};
use crate::tx::WriteTx;
use crate::{Commit, Snapshot, StoreError};

/// Which jobs run first (SPEC UI state, Ordering).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    /// UI actions and first loads.
    Ui,
    /// Sync and queue results.
    Sync,
    /// Everything else: contacts backfill, cleanup.
    Background,
}

impl Priority {
    const fn lane(self) -> usize {
        match self {
            Self::Ui => 0,
            Self::Sync => 1,
            Self::Background => 2,
        }
    }
}

/// A queued job: it runs on the writer thread, in its own transaction.
type Job<S> = dyn FnOnce(&mut Runner<S>) + Send;
/// Runs after every commit, on the writer thread, with the caller's state,
/// the committed database, and what the commit changed.
type OnCommit<S> = dyn FnMut(&mut S, &Snapshot<'_>, &Commit) + Send;

enum Entry<S> {
    Job(Box<Job<S>>),
    Stop(reply::Sender<Result<(), StoreError>>),
}

struct Queue<S> {
    lanes: [VecDeque<Entry<S>>; 3],
    /// No more jobs are taken: every handle is gone, or `stop` ran.
    closed: bool,
}

impl<S> Queue<S> {
    fn pop(&mut self) -> Option<Entry<S>> {
        self.lanes.iter_mut().find_map(VecDeque::pop_front)
    }
}

struct Shared<S> {
    queue: Mutex<Queue<S>>,
    ready: Condvar,
}

impl<S> Shared<S> {
    /// Queues an entry; gives it back when the writer has stopped.
    fn push(&self, priority: Priority, entry: Entry<S>) -> Result<(), Entry<S>> {
        {
            let mut queue = self.queue.lock();
            if queue.closed {
                return Err(entry);
            }
            if let Some(lane) = queue.lanes.get_mut(priority.lane()) {
                lane.push_back(entry);
            }
        }
        self.ready.notify_one();
        Ok(())
    }

    fn close(&self) {
        self.queue.lock().closed = true;
        self.ready.notify_one();
    }
}

/// Closes the queue when the last [`Writer`] clone goes. The thread then runs
/// what is queued and exits.
struct Handle<S> {
    shared: Arc<Shared<S>>,
    thread: ThreadId,
}

impl<S> Drop for Handle<S> {
    fn drop(&mut self) {
        self.shared.close();
    }
}

/// The handle to the writer thread. Clones share one thread.
///
/// `S` is the caller's writer-side state (core's subscription registry and
/// effect buffer). It lives on the writer thread, so jobs and `on_commit`
/// reach it without a lock.
pub struct Writer<S> {
    handle: Arc<Handle<S>>,
}

impl<S> Clone for Writer<S> {
    fn clone(&self) -> Self {
        Self {
            handle: Arc::clone(&self.handle),
        }
    }
}

impl<S: Send + 'static> Writer<S> {
    /// Opens the database at `path` (creating it, or moving aside one with
    /// another schema version) and starts the writer thread with `state`.
    ///
    /// After each commit, `on_commit` runs on the writer thread with the
    /// state, a [`Snapshot`] of what was just committed, and the [`Commit`],
    /// before the next job starts and before the job's caller gets its
    /// result. After each job whose transaction rolled back (it returned
    /// `Err`, panicked, or its commit failed), `on_rollback` runs instead,
    /// so the state can drop what it buffered for that job.
    pub fn start(
        path: &Path,
        state: S,
        on_commit: impl FnMut(&mut S, &Snapshot<'_>, &Commit) + Send + 'static,
        on_rollback: impl FnMut(&mut S) + Send + 'static,
    ) -> Result<Self, StoreError> {
        let conn = crate::open::open_writer(path)?;
        let rev = meta::stored_rev(&conn)?;
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue {
                lanes: [VecDeque::new(), VecDeque::new(), VecDeque::new()],
                closed: false,
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
        let thread = std::thread::Builder::new()
            .name("ub-writer".to_owned())
            .spawn(move || run(&thread_shared, runner))
            .map_err(|source| StoreError::Io {
                action: "start the writer thread for",
                path: path.to_path_buf(),
                source,
            })?;
        Ok(Self {
            handle: Arc::new(Handle {
                shared,
                thread: thread.thread().id(),
            }),
        })
    }

    /// Queues a job and returns its result to await. The job runs in its own
    /// transaction: on `Ok` it commits, on `Err` or a panic it rolls back.
    /// The result arrives once the commit and `on_commit` are done, so a
    /// read after it sees the job's writes.
    pub fn call<T, E, F>(&self, priority: Priority, job: F) -> Pending<T, E>
    where
        T: Send + 'static,
        E: From<StoreError> + Send + 'static,
        F: FnOnce(&mut WriteTx<'_>, &mut S) -> Result<T, E> + Send + 'static,
    {
        let (tx, rx) = reply::channel();
        let entry = Entry::Job(Box::new(move |runner: &mut Runner<S>| {
            tx.send(runner.transact(job));
        }));
        // Refused: dropping the job drops its sender, so the reply reads as
        // "the writer has stopped".
        drop(self.handle.shared.push(priority, entry));
        Pending::new(rx)
    }

    /// [`Writer::call`] for a thread that may block. Fails with
    /// [`StoreError::Reentrant`] on the writer thread itself, where waiting
    /// would never end.
    pub fn call_blocking<T, E, F>(&self, priority: Priority, job: F) -> Result<T, E>
    where
        T: Send + 'static,
        E: From<StoreError> + Send + 'static,
        F: FnOnce(&mut WriteTx<'_>, &mut S) -> Result<T, E> + Send + 'static,
    {
        if std::thread::current().id() == self.handle.thread {
            return Err(StoreError::Reentrant.into());
        }
        self.call(priority, job).wait()
    }

    /// Stops the writer after the UI jobs queued before this call. Queued
    /// sync and background jobs are dropped, and their callers get
    /// [`StoreError::WriterStopped`], as does every later call. Resolves
    /// once the database is closed.
    pub fn stop(&self) -> Pending<(), StoreError> {
        let (tx, rx) = reply::channel();
        if let Err(Entry::Stop(tx)) = self.handle.shared.push(Priority::Ui, Entry::Stop(tx)) {
            // Already stopped: done.
            tx.send(Ok(()));
        }
        Pending::new(rx)
    }
}

/// The writer thread's own state.
struct Runner<S> {
    conn: Connection,
    /// The last committed `rev`.
    rev: Rev,
    state: S,
    on_commit: Box<OnCommit<S>>,
    /// Runs after every rolled-back job.
    on_rollback: Box<dyn FnMut(&mut S) + Send>,
}

impl<S> Runner<S> {
    /// Runs one job in one transaction.
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
        let tx = match conn.transaction_with_behavior(TransactionBehavior::Immediate) {
            Ok(tx) => tx,
            Err(e) => {
                rolled_back(on_rollback, state);
                return Err(StoreError::sql("begin a write transaction")(e).into());
            }
        };
        let next = Rev(rev.0.saturating_add(1));
        let mut write = WriteTx::new(&tx, next);
        let outcome = catch_unwind(AssertUnwindSafe(|| job(&mut write, &mut *state)));
        let value = match outcome {
            Ok(Ok(value)) => value,
            Ok(Err(e)) => {
                drop(write);
                drop(tx);
                rolled_back(on_rollback, state);
                return Err(e);
            }
            Err(panic) => {
                let message = panic_message(panic.as_ref());
                tracing::error!(target: "ub_store", %message, "writer job panicked; rolled back");
                drop(write);
                drop(tx);
                rolled_back(on_rollback, state);
                return Err(StoreError::JobPanicked(message).into());
            }
        };
        let finished = write.finish();
        let committed = (|| {
            if finished.wrote {
                meta::store_rev(&tx, next)?;
            }
            #[cfg(any(test, feature = "test-hooks"))]
            if finished.fail_commit {
                meta::break_commit(&tx)?;
            }
            tx.commit()
                .map_err(StoreError::sql("commit a write transaction"))
        })();
        if let Err(e) = committed {
            tracing::warn!(target: "ub_store", error = %e, "write transaction failed to commit");
            rolled_back(on_rollback, state);
            return Err(e.into());
        }
        if finished.wrote {
            *rev = next;
        }
        let commit = Commit {
            rev: *rev,
            changes: finished.changes,
        };
        let snapshot = Snapshot::new(conn);
        if let Err(panic) = catch_unwind(AssertUnwindSafe(|| {
            on_commit(&mut *state, &snapshot, &commit);
        })) {
            let message = panic_message(panic.as_ref());
            tracing::error!(target: "ub_store", %message, rev = commit.rev.0, "on_commit panicked after a commit");
        }
        Ok(value)
    }
}

fn rolled_back<S>(on_rollback: &mut dyn FnMut(&mut S), state: &mut S) {
    if let Err(panic) = catch_unwind(AssertUnwindSafe(|| on_rollback(state))) {
        let message = panic_message(panic.as_ref());
        tracing::error!(target: "ub_store", %message, "on_rollback panicked");
    }
}

fn panic_message(panic: &(dyn Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic with no message".to_owned())
}

/// The writer thread: runs entries until the queue closes and empties, or a
/// stop arrives.
fn run<S>(shared: &Shared<S>, mut runner: Runner<S>) {
    loop {
        let entry = {
            let mut queue = shared.queue.lock();
            loop {
                if let Some(entry) = queue.pop() {
                    break entry;
                }
                if queue.closed {
                    return;
                }
                shared.ready.wait(&mut queue);
            }
        };
        match entry {
            Entry::Job(job) => job(&mut runner),
            Entry::Stop(reply) => {
                let dropped: Vec<Entry<S>> = {
                    let mut queue = shared.queue.lock();
                    queue.closed = true;
                    queue.lanes.iter_mut().flat_map(std::mem::take).collect()
                };
                // Outside the lock: dropping a job drops its reply sender,
                // which wakes its caller.
                drop(dropped);
                drop(runner);
                reply.send(Ok(()));
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Subscriptions waiting for their first load and the first loads sent,
    /// as core's registry keeps them.
    #[derive(Default)]
    struct Registry {
        waiting: Vec<&'static str>,
        sent: Vec<&'static str>,
        rollbacks: u32,
    }

    fn start(dir: &tempfile::TempDir) -> Writer<Registry> {
        Writer::start(
            &dir.path().join("mail.db"),
            Registry::default(),
            |registry: &mut Registry, _: &Snapshot<'_>, _: &Commit| {
                let waiting = std::mem::take(&mut registry.waiting);
                registry.sent.extend(waiting);
            },
            |registry: &mut Registry| registry.rollbacks += 1,
        )
        .unwrap()
    }

    #[test]
    fn a_failed_commit_rolls_back_and_its_first_load_goes_with_the_next_commit() {
        let dir = tempfile::tempdir().unwrap();
        let writer = start(&dir);
        let failed: Result<(), StoreError> = writer.call_blocking(Priority::Ui, |tx, registry| {
            tx.accounts().create("a@example.com", "A")?;
            registry.waiting.push("app");
            tx.fail_commit();
            Ok(())
        });
        assert!(
            matches!(&failed, Err(StoreError::Sqlite { what, .. }) if what.contains("commit")),
            "{failed:?}"
        );
        let (accounts, rollbacks, sent) = writer
            .call_blocking(Priority::Ui, |tx, registry| {
                Ok::<_, StoreError>((
                    tx.read().accounts().all()?.len(),
                    registry.rollbacks,
                    registry.sent.clone(),
                ))
            })
            .unwrap();
        assert_eq!(accounts, 0, "the failed commit's write is gone");
        assert_eq!(rollbacks, 1);
        assert!(sent.is_empty(), "nothing went out with the failed commit");
        // That read job's commit sent the first load the failed one lost.
        let sent = writer
            .call_blocking(Priority::Ui, |_, registry| {
                Ok::<_, StoreError>(registry.sent.clone())
            })
            .unwrap();
        assert_eq!(sent, ["app"], "sent once, after the failure");
        let rev = writer
            .call_blocking(Priority::Ui, |tx, _| tx.read().meta().rev())
            .unwrap();
        assert_eq!(rev, Rev(0), "rev did not move");
    }
}
