use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use rusqlite::Connection;
use supermonkey_lib::domain::{Rev, SettingKey};
use supermonkey_lib::store::{Priority, ReadPool, SCHEMA_VERSION, StoreError, Writer, open_writer};

#[test]
fn writer_savepoints_rollbacks_panics_rev_and_idempotent_stop() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("mail.db");
    let rollbacks = Arc::new(AtomicUsize::new(0));
    let r = Arc::clone(&rollbacks);

    let writer = Writer::start(
        &db_path,
        (),
        |(), _db, _commit| {},
        move |()| {
            r.fetch_add(1, Ordering::SeqCst);
        },
    )
    .unwrap();

    let id = writer
        .call(Priority::Ui, |tx, ()| {
            let a = tx.create_account("a@example.com", "A")?;
            let err: Result<(), StoreError> = tx.savepoint(|tx| {
                tx.create_account("rollback@example.com", "R")?;
                Err(StoreError::Invalid("step failed"))
            });
            assert!(err.is_err());
            Ok::<_, StoreError>(a.id)
        })
        .wait()
        .unwrap();

    let pool = ReadPool::open(&db_path, NonZeroUsize::new(2).unwrap()).unwrap();
    let accounts = read_accounts(&pool);
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].id, id);

    let failed: Result<(), StoreError> = writer
        .call(Priority::Ui, |tx, ()| {
            tx.create_account("bad@example.com", "Bad")?;
            Err(StoreError::Invalid("abort commit"))
        })
        .wait();
    assert!(failed.is_err());
    assert_eq!(read_accounts(&pool).len(), 1);
    assert_eq!(read_rev(&pool), Rev(1));
    assert_eq!(rollbacks.load(Ordering::SeqCst), 1);

    let panicked: Result<(), StoreError> =
        writer.call(Priority::Sync, |_tx, ()| panic!("boom")).wait();
    assert!(matches!(panicked, Err(StoreError::JobPanicked(_))));
    assert_eq!(rollbacks.load(Ordering::SeqCst), 2);

    let s1 = writer.stop();
    let s2 = writer.stop();
    assert!(s1.wait().is_ok());
    assert!(s2.wait().is_ok());
}

#[expect(
    clippy::redundant_closure_for_method_calls,
    clippy::unwrap_used,
    reason = "test helper closure preserves higher-ranked lifetime over Db<'_>"
)]
fn read_accounts(pool: &ReadPool) -> Vec<supermonkey_lib::store::AccountRecord> {
    pool.with(|db| db.accounts()).unwrap()
}

#[expect(
    clippy::redundant_closure_for_method_calls,
    clippy::unwrap_used,
    reason = "test helper closure preserves higher-ranked lifetime over Db<'_>"
)]
fn read_rev(pool: &ReadPool) -> Rev {
    pool.with(|db| db.rev()).unwrap()
}

#[test]
fn open_writer_moves_aside_wrong_version_or_corrupt_db_and_fails_on_unopenable() {
    let dir = tempfile::tempdir().unwrap();
    let corrupt = dir.path().join("corrupt.db");
    std::fs::write(&corrupt, b"not a sqlite file").unwrap();
    let conn = open_writer(&corrupt).unwrap();
    let ver: i32 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(ver, SCHEMA_VERSION);
    assert!(dir.path().join("corrupt.db.old-1").exists());
    drop(conn);

    let old = dir.path().join("old.db");
    let raw = Connection::open(&old).unwrap();
    raw.pragma_update(None, "user_version", SCHEMA_VERSION - 1)
        .unwrap();
    drop(raw);
    let fresh = open_writer(&old).unwrap();
    let ver: i32 = fresh
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(ver, SCHEMA_VERSION);
    assert!(dir.path().join("old.db.old-1").exists());
    drop(fresh);

    let dir_as_db = dir.path().join("dir.db");
    std::fs::create_dir_all(&dir_as_db).unwrap();
    assert!(open_writer(&dir_as_db).is_err());
    assert!(dir_as_db.is_dir());

    let valid = dir.path().join("valid.db");
    let w = Writer::start(&valid, (), |(), _, _| {}, |()| {}).unwrap();
    w.call(Priority::Ui, |tx, ()| {
        let a = tx.create_account("t@example.com", "T")?;
        tx.set_setting(Some(a.id), SettingKey::Muted, true)?;
        assert!(tx.read().setting_is_on(Some(a.id), SettingKey::Muted)?);
        Ok::<_, StoreError>(())
    })
    .wait()
    .unwrap();
    w.stop().wait().unwrap();
}

#[test]
fn open_writer_leaves_locked_database_in_place_and_fails() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("locked.db");
    let conn = open_writer(&path).unwrap();
    drop(conn);

    let locker = Connection::open(&path).unwrap();
    locker
        .execute_batch(
            "PRAGMA locking_mode = EXCLUSIVE; BEGIN EXCLUSIVE; UPDATE meta SET rev = rev;",
        )
        .unwrap();
    let err = open_writer(&path);
    assert!(err.is_err(), "locked database fails to open");
    assert!(path.exists(), "locked database file stays in place");
    assert!(
        !dir.path().join("locked.db.old-1").exists(),
        "locked database is never moved aside"
    );
    drop(locker);

    assert!(
        open_writer(&path).is_ok(),
        "opens cleanly once lock releases"
    );
}
