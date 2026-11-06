//! The writer actor: ordering, transactions, rev, hooks, panics, stopping.

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::sync_channel;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::Thread;

use ub_domain::{ActionNo, Rev};
use ub_store::{Pending, Priority, StoreError, Table, WriteTx};

use crate::common::{Harness, State, fetched, msg};

fn tag(name: &'static str) -> impl FnOnce(&mut WriteTx<'_>, &mut State) -> Result<(), StoreError> {
    move |_, state| {
        state.order.push(name.to_owned());
        Ok(())
    }
}

/// Holds the writer busy until the returned sender sends.
fn hold(h: &Harness) -> (std::sync::mpsc::SyncSender<()>, Pending<(), StoreError>) {
    let (started_tx, started) = sync_channel(0);
    let (release, release_rx) = sync_channel::<()>(0);
    let held = h.writer.call(Priority::Ui, move |_, _: &mut State| {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        Ok(())
    });
    started.recv().unwrap();
    (release, held)
}

#[test]
fn jobs_run_by_priority_then_in_arrival_order() {
    let h = Harness::start();
    let (release, held) = hold(&h);
    let queued: Vec<_> = [
        (Priority::Background, "background 1"),
        (Priority::Sync, "sync 1"),
        (Priority::Ui, "ui 1"),
        (Priority::Background, "background 2"),
        (Priority::Ui, "ui 2"),
        (Priority::Sync, "sync 2"),
    ]
    .into_iter()
    .map(|(priority, name)| h.writer.call(priority, tag(name)))
    .collect();
    release.send(()).unwrap();
    held.wait().unwrap();
    for job in queued {
        job.wait().unwrap();
    }
    let order = h
        .writer
        .call_blocking(Priority::Ui, |_, s: &mut State| {
            Ok::<_, StoreError>(s.order.clone())
        })
        .unwrap();
    assert_eq!(
        order,
        [
            "ui 1",
            "ui 2",
            "sync 1",
            "sync 2",
            "background 1",
            "background 2"
        ]
    );
}

#[test]
fn a_failed_job_rolls_back_and_skips_on_commit() {
    let h = Harness::start();
    let commits = h.commits.lock().len();
    let result: Result<(), StoreError> = h.writer.call_blocking(Priority::Ui, |tx, _| {
        tx.accounts().create("a@example.com", "A")?;
        Err(StoreError::Invalid("refused".to_owned()))
    });
    assert!(matches!(result, Err(StoreError::Invalid(_))), "{result:?}");
    assert_eq!(h.commits.lock().len(), commits, "no on_commit");
    assert_eq!(h.rollbacks.load(Ordering::SeqCst), 1, "on_rollback ran");
    assert!(
        h.read(|r| r.accounts().all()).is_empty(),
        "the write is gone"
    );
    assert_eq!(h.read(|r| r.meta().rev()), Rev(0), "rev did not move");
}

#[test]
fn rev_moves_once_per_writing_commit_and_survives_a_restart() {
    let mut h = Harness::start();
    assert_eq!(h.read(|r| r.meta().rev()), Rev(0));
    let account = h.account("a@example.com");
    let first = h.last_commit();
    assert_eq!(first.rev, Rev(1));
    assert!(first.changes.touched(account, Table::Account));

    // A job that writes nothing still commits and runs on_commit, at the same rev.
    let commits = h.commits.lock().len();
    h.write(|_| Ok(()));
    assert_eq!(h.commits.lock().len(), commits + 1);
    assert_eq!(h.last_commit().rev, Rev(1));
    assert!(h.last_commit().changes.is_empty());

    // The action number is not a row: it moves no rev, and never goes back.
    h.write(|tx| tx.meta().set_last_action(ActionNo(5)));
    h.write(|tx| tx.meta().set_last_action(ActionNo(3)));
    assert_eq!(h.last_commit().rev, Rev(1));

    h.restart();
    assert_eq!(h.read(|r| r.meta().rev()), Rev(1));
    assert_eq!(h.read(|r| r.meta().last_action()), ActionNo(5));
    h.account("b@example.com");
    assert_eq!(h.last_commit().rev, Rev(2), "continues after the restart");
}

#[test]
fn a_commit_lists_and_stamps_what_it_wrote() {
    let h = Harness::start();
    let account = h.account("me@example.com");
    let stored = h.write(move |tx| {
        tx.threads().store(&fetched(
            account,
            "t1",
            vec![
                msg("m1", 1_000, "alice@example.com", &["INBOX"]),
                msg("m2", 2_000, "bob@example.com", &["INBOX"]),
            ],
        ))
    });
    let commit = h.last_commit();
    let thread = stored.thread.unwrap();
    assert_eq!(commit.changes.threads, [thread].into());
    assert_eq!(
        commit.changes.messages,
        stored.added.iter().copied().collect()
    );
    assert_eq!(stored.added.len(), 2);
    assert_eq!(
        commit.changes.tables,
        [(account, Table::Threads)].into(),
        "a thread's messages are its thread's"
    );
    let row = h.read(move |r| r.threads().row(thread)).unwrap();
    assert_eq!(row.rev, commit.rev);
    for id in stored.added {
        assert_eq!(
            h.read(move |r| r.messages().get(id)).unwrap().rev,
            commit.rev
        );
    }
}

#[test]
fn a_panicking_job_rolls_back_and_the_writer_keeps_going() {
    let h = Harness::start();
    let result: Result<(), StoreError> = h.writer.call_blocking(Priority::Ui, |tx, _| {
        tx.accounts().create("a@example.com", "A")?;
        panic!("boom");
    });
    match result {
        Err(StoreError::JobPanicked(message)) => assert_eq!(message, "boom"),
        other => panic!("expected a panic error, got {other:?}"),
    }
    assert_eq!(h.rollbacks.load(Ordering::SeqCst), 1);
    assert!(h.read(|r| r.accounts().all()).is_empty(), "rolled back");
    let account = h.account("b@example.com");
    assert_eq!(
        h.read(move |r| r.accounts().get(account)).unwrap().email,
        "b@example.com"
    );
}

#[test]
fn stop_runs_earlier_ui_jobs_and_drops_queued_sync_jobs() {
    let h = Harness::start();
    let (release, held) = hold(&h);
    let ui = h.writer.call(Priority::Ui, tag("ui"));
    let sync = h.writer.call(Priority::Sync, tag("sync"));
    let stopped = h.writer.stop();
    release.send(()).unwrap();
    held.wait().unwrap();
    ui.wait().unwrap();
    assert!(matches!(sync.wait(), Err(StoreError::WriterStopped)));
    stopped.wait().unwrap();
    let late: Result<(), StoreError> = h.writer.call_blocking(Priority::Ui, tag("late"));
    assert!(matches!(late, Err(StoreError::WriterStopped)));
    // Stopping twice is fine.
    h.writer.stop().wait().unwrap();
}

#[test]
fn waiting_on_the_writer_thread_is_refused() {
    let h = Harness::start();
    let writer = h.writer.clone();
    let inner = h
        .writer
        .call_blocking(Priority::Ui, move |_, _: &mut State| {
            Ok::<_, StoreError>(writer.call_blocking(Priority::Ui, tag("inner")))
        })
        .unwrap();
    assert!(matches!(inner, Err(StoreError::Reentrant)));
}

#[test]
fn a_result_can_be_awaited() {
    struct Unpark(Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = pin!(future);
        let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
        let mut cx = Context::from_waker(&waker);
        loop {
            if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
                return value;
            }
            std::thread::park();
        }
    }

    let h = Harness::start();
    let id = block_on(h.writer.call(Priority::Sync, |tx, _: &mut State| {
        tx.accounts().create("a@example.com", "A")
    }))
    .unwrap();
    assert_eq!(
        h.read(move |r| r.accounts().get(id)).unwrap().email,
        "a@example.com"
    );
}

#[test]
fn the_pool_reads_what_was_committed() {
    let h = Harness::start();
    let pool =
        ub_store::ReadPool::open(&h.path(), std::num::NonZeroUsize::new(2).unwrap()).unwrap();
    let count = || pool.with(|r| r.accounts().all()).unwrap().len();
    assert_eq!(count(), 0);
    h.account("a@example.com");
    assert_eq!(count(), 1, "visible once the call returned");
    // Nested reads take the other connection instead of waiting.
    let nested = pool
        .with(|outer| {
            let inner = pool.with(|inner| inner.accounts().all())?;
            Ok::<_, StoreError>((outer.accounts().all()?.len(), inner.len()))
        })
        .unwrap();
    assert_eq!(nested, (1, 1));
}

#[test]
fn a_failed_savepoint_undoes_only_its_own_writes_and_changes() {
    let h = Harness::start();
    let (kept, failed) = h.write(|tx| {
        let kept = tx.savepoint(|tx| tx.accounts().create("kept@example.com", "Kept"))?;
        let failed: Result<ub_domain::AccountId, StoreError> = tx.savepoint(|tx| {
            tx.accounts().create("gone@example.com", "Gone")?;
            Err(StoreError::Invalid("refused on purpose".to_owned()))
        });
        Ok((kept, failed.is_err()))
    });
    assert!(failed);
    let commit = h.last_commit();
    assert!(
        commit
            .changes
            .tables
            .iter()
            .all(|&(account, _)| account == kept),
        "the failed step's changes are gone from the commit: {:?}",
        commit.changes.tables
    );
    assert_eq!(commit.rev, Rev(1), "a fresh database's first write");
    let emails: Vec<String> = h
        .read(|r| r.accounts().all())
        .into_iter()
        .map(|a| a.email)
        .collect();
    assert_eq!(emails, ["kept@example.com"]);
}

#[test]
fn a_job_whose_only_writes_rolled_back_in_a_savepoint_does_not_move_rev() {
    let h = Harness::start();
    h.account("a@example.com");
    let before = h.last_commit().rev;
    h.write(|tx| {
        let failed: Result<(), StoreError> = tx.savepoint(|tx| {
            tx.accounts().create("gone@example.com", "Gone")?;
            Err(StoreError::Invalid("refused on purpose".to_owned()))
        });
        assert!(failed.is_err());
        Ok(())
    });
    let commit = h.last_commit();
    assert_eq!(commit.rev, before);
    assert!(commit.changes.is_empty());
}
