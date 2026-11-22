use std::any::Any;
use std::collections::VecDeque;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::Arc;

use parking_lot::{Condvar, Mutex};
use rusqlite::{Connection, Transaction, TransactionBehavior};
use sm_domain::Rev;

use crate::meta;
use crate::reply::{self, Pending};
use crate::tx::{Finished, WriteTx};
use crate::{Commit, Snapshot, StoreError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    Ui,
    Sync,
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

type Job<S> = dyn FnOnce(&mut Runner<S>) + Send;
type OnCommit<S> = dyn FnMut(&mut S, &Snapshot<'_>, &Commit) + Send;

enum Entry<S> {
    Job(Box<Job<S>>),
    Stop(reply::Sender<Result<(), StoreError>>),
}

struct Queue<S> {
    lanes: [VecDeque<Entry<S>>; 3],
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
        let (tx, rx) = reply::channel();
        let entry = Entry::Job(Box::new(move |runner: &mut Runner<S>| {
            tx.send(runner.transact(job));
        }));
        drop(self.handle.shared.push(priority, entry));
        Pending::new(rx)
    }

    pub fn stop(&self) -> Pending<(), StoreError> {
        let (tx, rx) = reply::channel();
        if let Err(Entry::Stop(tx)) = self.handle.shared.push(Priority::Ui, Entry::Stop(tx)) {
            tx.send(Ok(()));
        }
        Pending::new(rx)
    }
}

struct Runner<S> {
    conn: Connection,
    rev: Rev,
    state: S,
    on_commit: Box<OnCommit<S>>,
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
        let tx = match conn.transaction_with_behavior(TransactionBehavior::Immediate) {
            Ok(tx) => tx,
            Err(e) => {
                rolled_back(on_rollback, state);
                return Err(StoreError::sql("begin a write transaction")(e).into());
            }
        };
        let next = Rev(rev.0.saturating_add(1));
        let mut write = WriteTx::new(&tx, next);
        let value = match run_job(&mut write, state, job) {
            Ok(value) => value,
            Err(e) => {
                drop(write);
                drop(tx);
                rolled_back(on_rollback, state);
                return Err(e);
            }
        };
        let finished = write.finish();
        if let Err(e) = commit_tx(tx, next, &finished) {
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
        committed(on_commit, state, conn, &commit);
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
            tracing::error!(target: "sm_store", %message, "writer job panicked; rolled back");
            Err(StoreError::JobPanicked(message).into())
        }
    }
}

fn commit_tx(tx: Transaction<'_>, next: Rev, finished: &Finished) -> Result<(), StoreError> {
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
    if let Err(e) = &committed {
        tracing::warn!(target: "sm_store", error = %e, "write transaction failed to commit");
    }
    committed
}

fn committed<S>(
    on_commit: &mut dyn FnMut(&mut S, &Snapshot<'_>, &Commit),
    state: &mut S,
    conn: &Connection,
    commit: &Commit,
) {
    let snapshot = Snapshot::new(conn);
    if let Err(panic) = catch_unwind(AssertUnwindSafe(|| {
        on_commit(state, &snapshot, commit);
    })) {
        let message = panic_message(panic.as_ref());
        tracing::error!(target: "sm_store", %message, rev = commit.rev.0, "on_commit panicked after a commit");
    }
}

fn rolled_back<S>(on_rollback: &mut dyn FnMut(&mut S), state: &mut S) {
    if let Err(panic) = catch_unwind(AssertUnwindSafe(|| on_rollback(state))) {
        let message = panic_message(panic.as_ref());
        tracing::error!(target: "sm_store", %message, "on_rollback panicked");
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
        let failed: Result<(), StoreError> = writer
            .call(Priority::Ui, |tx, registry| {
                tx.accounts().create("a@example.com", "A")?;
                registry.waiting.push("app");
                tx.fail_commit();
                Ok(())
            })
            .wait();
        assert!(
            matches!(&failed, Err(StoreError::Sqlite { what, .. }) if what.contains("commit")),
            "{failed:?}"
        );
        let (accounts, rollbacks, sent) = writer
            .call(Priority::Ui, |tx, registry| {
                Ok::<_, StoreError>((
                    tx.read().accounts().all()?.len(),
                    registry.rollbacks,
                    registry.sent.clone(),
                ))
            })
            .wait()
            .unwrap();
        assert_eq!(accounts, 0, "the failed commit's write is gone");
        assert_eq!(rollbacks, 1);
        assert!(sent.is_empty(), "nothing went out with the failed commit");
        let sent = writer
            .call(Priority::Ui, |_, registry| {
                Ok::<_, StoreError>(registry.sent.clone())
            })
            .wait()
            .unwrap();
        assert_eq!(sent, ["app"], "sent once, after the failure");
        let rev = writer
            .call(Priority::Ui, |tx, _| tx.read().meta().rev())
            .wait()
            .unwrap();
        assert_eq!(rev, Rev(0), "rev did not move");
    }
}
