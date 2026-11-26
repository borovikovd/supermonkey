use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use rusqlite::Connection;
use supermonkey_lib::domain::query::Query;
use supermonkey_lib::domain::{
    AccountId, AttachmentId, DraftId, GmailAttachmentId, Millis, OpKind, OutboxState, PinKind, Rev,
    SettingKey, SplitKind, ThreadId,
};
use supermonkey_lib::store::{
    LEGACY_DB_FILE, Priority, ReadPool, SCHEMA_VERSION, StoreError, UndoAction, Writer,
    import_legacy, open_writer,
};

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

#[test]
#[expect(
    clippy::too_many_lines,
    clippy::cognitive_complexity,
    reason = "one pass over everything a 0.7.2 database holds, table by table"
)]
fn the_database_of_0_7_2_is_imported_into_the_shapes_this_version_reads() {
    let dir = tempfile::tempdir().unwrap();
    let earlier = dir.path().join(LEGACY_DB_FILE);
    std::fs::copy("tests/fixtures/data-0.7.2/supermonkey.db", &earlier).unwrap();
    let target = dir.path().join("mail.db");

    import_legacy(&earlier, &target).unwrap();
    assert!(!earlier.exists(), "the earlier file is moved aside");
    assert!(dir.path().join("supermonkey.db.old-1").exists());
    drop(open_writer(&target).unwrap());

    let pool = ReadPool::open(&target, NonZeroUsize::new(1).unwrap()).unwrap();
    let (ana, ben) = (AccountId(1), AccountId(2));
    pool.with(|db| {
        let accounts = db.accounts()?;
        let emails: Vec<&str> = accounts.iter().map(|a| a.email.as_str()).collect();
        assert_eq!(emails, vec!["ana@acme.test", "ben@bolt.test"]);
        assert!(accounts.iter().all(|a| a.connected && !a.needs_signin));
        let identity = accounts[0].identity();
        assert!(identity.is_mine("ana@ames.dev"), "the alias is kept");
        assert_eq!(identity.default_from().email, "ana@acme.test");
        assert_eq!(db.last_action()?.0, 66);
        assert_eq!(db.rev()?, Rev(121));

        let splits = db.splits(ana)?;
        let placed: Vec<(&str, SplitKind, i32)> = splits
            .iter()
            .map(|s| (s.name.as_str(), s.kind, s.position))
            .collect();
        assert_eq!(
            placed,
            vec![
                ("VIP", SplitKind::Custom, 0),
                ("Important", SplitKind::Important, 10_000),
                ("Other", SplitKind::Other, 20_000),
            ]
        );
        let pins = db.pins(ana)?;
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].kind, PinKind::Sender);
        assert_eq!(pins[0].value, "billing@shop.test");
        assert_eq!(pins[0].split, splits[0].id);

        let snoozed = |id: i64| db.thread_row(ThreadId(id)).map(|t| t.and_then(|t| t.snooze_until));
        assert_eq!(snoozed(13)?, Some(Millis(1_793_973_600_000)));
        assert_eq!(snoozed(12)?, Some(Millis(1_794_229_200_000)));
        assert!(
            db.thread_row(ThreadId(44))?.is_none(),
            "a thread that held only a draft copy is gone"
        );
        for id in 1..=80 {
            if db.thread_row(ThreadId(id))?.is_some() {
                assert!(
                    !db.thread_messages(ThreadId(id))?.is_empty(),
                    "thread {id} reads back with its messages"
                );
            }
        }
        let quokka = db.thread_messages(ThreadId(8))?;
        assert_eq!(
            quokka[0].message_id_hdr.as_deref(),
            Some("<18c041a900000005@fake.example>")
        );
        assert_eq!(
            db.search_threads(ana, &Query::parse("quokka"), &[])?,
            vec![ThreadId(8)]
        );

        let drafts = db.drafts_for_account(ana)?;
        assert_eq!(drafts.len(), 10);
        let reply = drafts
            .iter()
            .find(|d| d.subject == "Re: Person 3")
            .unwrap();
        assert_eq!(
            reply.in_reply_to.as_deref(),
            Some("<18c041a900000005@fake.example>")
        );
        assert_eq!(reply.references, vec!["<18c041a900000005@fake.example>"]);
        assert!(reply.lifecycle.gmail_thread_id.is_some());
        let forward = drafts
            .iter()
            .find(|d| d.subject == "Fwd: Report to forward")
            .unwrap();
        assert_eq!(forward.attachments.len(), 1);
        assert_eq!(forward.attachments[0].filename, "report.pdf");
        assert_eq!(forward.attachments[0].mime_type, "application/pdf");
        assert_eq!(forward.attachments[0].source, Some(AttachmentId(4)));
        let from_gmail = drafts
            .iter()
            .find(|d| d.subject == "Re: Person 4")
            .unwrap();
        assert_eq!(from_gmail.id.as_str(), "g-r-18c041a90000007d");
        assert_eq!(from_gmail.lifecycle.waits_for, None);
        assert_eq!(from_gmail.attachments[0].filename, "spec.pdf");
        assert_eq!(
            from_gmail.attachments[0].source, None,
            "the stored copy of the Gmail draft is gone, so the file is fetched from Gmail"
        );
        assert_eq!(
            from_gmail.attachments[0]
                .gmail_att_id
                .as_ref()
                .map(GmailAttachmentId::as_str),
            Some("ANGjdJ_18c041a90000007c_1")
        );

        let outbox = db.outbox_all()?;
        assert_eq!(outbox.len(), 1);
        assert_eq!(outbox[0].item.state, OutboxState::Waiting);
        assert!(outbox[0].item.delayed);

        let ops = db.all_ops(ana)?;
        let kinds: Vec<OpKind> = ops.iter().map(|o| o.kind).collect();
        assert_eq!(
            kinds,
            vec![
                OpKind::Modify,
                OpKind::Modify,
                OpKind::Modify,
                OpKind::Modify,
                OpKind::CreateLabel,
                OpKind::Modify,
                OpKind::Modify,
                OpKind::DraftUpsert,
                OpKind::Modify,
                OpKind::DraftUpsert,
                OpKind::DraftUpsert,
            ]
        );
        let snooze: serde_json::Value = ops[0].parse_payload()?;
        assert_eq!(
            snooze,
            serde_json::json!({
                "thread": 12,
                "gmailMsgIds": ["18c041a90000000c"],
                "add": [],
                "remove": ["INBOX"],
                "revert": {
                    "thread": 12,
                    "messages": [{
                        "message": 14,
                        "gmail_id": "18c041a90000000c",
                        "added": [],
                        "removed": ["INBOX"],
                    }],
                    "snooze_before": null,
                    "snooze_after": 1_794_229_200_000_i64,
                },
            })
        );
        let label: serde_json::Value = ops[4].parse_payload()?;
        assert_eq!(label["name"], "Upgrade kept");
        assert!(
            label["localId"]
                .as_str()
                .is_some_and(|id| id.starts_with("local-"))
        );
        let upsert: serde_json::Value = ops[7].parse_payload()?;
        assert_eq!(
            upsert["draftId"].as_str(),
            ops[7].draft_id.as_ref().map(DraftId::as_str)
        );

        let undo = db.undo_entries(ana)?;
        assert_eq!(undo.len(), 7);
        assert!(
            undo.iter()
                .any(|u| matches!(&u.action, UndoAction::Send { .. }))
        );
        assert!(undo.iter().any(|u| matches!(
            &u.action,
            UndoAction::Pin { previous: None, value, .. } if value == "billing@shop.test"
        )));
        assert!(undo.iter().any(|u| matches!(
            &u.action,
            UndoAction::Mail { threads } if threads[0].messages[0].gmail_id.as_str() == "18c041a90000000c"
        )));

        assert!(!db.setting_is_on(None, SettingKey::RemoteImages)?);
        assert!(db.setting_is_on(Some(ben), SettingKey::Muted)?);
        assert!(!db.setting_is_on(Some(ana), SettingKey::Muted)?);
        Ok::<_, StoreError>(())
    })
    .unwrap();
}
