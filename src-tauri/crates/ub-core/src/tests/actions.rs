//! Actions, the channel, and queries through a started core.

use std::time::Duration;

use ub_domain::{
    AccountId, ActionNo, Addr, DraftId, DraftKind, MessageId, Millis, NoticeKind, OutboxState,
    SubId, ThreadId, View,
};
use ub_ipc::{
    Action, ChannelMessage, DraftFields, DraftSave, ModifyAction, NumberedAction, SubKey, SubUpdate,
};
use ub_store::{Draft, NewBody, NewNotice, NewOutbox, Priority, StoreError};

use super::{ACCOUNT_EMAIL, Harness, eventually, seed};

fn sub(name: &str) -> SubId {
    SubId(name.to_owned())
}

fn subscribe(name: &str, key: SubKey) -> Action {
    Action::Subscribe {
        sub: sub(name),
        key,
    }
}

fn app_state(messages: &[ChannelMessage]) -> Option<ub_ipc::AppState> {
    messages
        .iter()
        .flat_map(|m| &m.updates)
        .filter_map(|u| match u {
            SubUpdate::App { state, .. } => Some(state.clone()),
            _ => None,
        })
        .next_back()
}

fn summary(messages: &[ChannelMessage]) -> Option<ub_ipc::AccountSummary> {
    messages
        .iter()
        .flat_map(|m| &m.updates)
        .filter_map(|u| match u {
            SubUpdate::Account { summary, .. } => Some(summary.clone()),
            _ => None,
        })
        .next_back()
}

fn a_modify(account: AccountId) -> Action {
    Action::Modify(ModifyAction {
        account,
        kind: ub_domain::ModifyKind::Done,
        threads: vec![ThreadId(1)],
        add: Vec::new(),
        remove: vec!["INBOX".into()],
        snooze: None,
        undoable: true,
    })
}

#[tokio::test]
async fn first_loads_of_one_batch_go_out_together_in_subscribe_order() {
    let mut h = Harness::start().await;
    let (account, _) = h.write(seed).await;
    let ns = h
        .act(vec![
            subscribe("s1", SubKey::App),
            subscribe("s2", SubKey::Account { account }),
            subscribe(
                "s3",
                SubKey::List {
                    account,
                    view: View::Inbox,
                },
            ),
        ])
        .await;
    let messages = h.take();
    let [message] = messages.as_slice() else {
        panic!("one message: {messages:?}");
    };
    assert_eq!(message.ack, ActionNo(ns[2]));
    let kinds: Vec<(&str, bool)> = message
        .updates
        .iter()
        .map(|u| match u {
            SubUpdate::App { sub, first, .. }
            | SubUpdate::Account { sub, first, .. }
            | SubUpdate::List { sub, first, .. }
            | SubUpdate::Conversation { sub, first, .. }
            | SubUpdate::Composer { sub, first, .. } => (sub.as_str(), *first),
        })
        .collect();
    assert_eq!(kinds, [("s1", true), ("s2", true), ("s3", true)]);
}

#[tokio::test]
async fn a_batch_that_changes_nothing_shown_still_moves_ack() {
    let mut h = Harness::start().await;
    let ns = h.act(vec![Action::Unsubscribe { sub: sub("never") }]).await;
    let messages = h.take();
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert_eq!(messages[0].ack, ActionNo(ns[0]));
    assert_eq!(messages[0].updates, []);
}

#[tokio::test]
async fn a_rejected_action_is_skipped_and_the_batch_goes_on() {
    let mut h = Harness::start().await;
    let (account, _) = h.write(seed).await;
    h.act(vec![subscribe("app", SubKey::App)]).await;
    h.take();
    let ns = h
        .act(vec![
            a_modify(account),
            Action::SetSetting {
                account: None,
                key: "remote_images".to_owned(),
                value: "off".to_owned(),
            },
            Action::SetSetting {
                account: None,
                key: "bogus".to_owned(),
                value: "on".to_owned(),
            },
        ])
        .await;
    let messages = h.take();
    assert_eq!(
        messages.last().map(|m| m.ack),
        Some(ActionNo(ns[2])),
        "ack moves past rejected actions"
    );
    let state = app_state(&messages).expect("the setting reached the app");
    assert!(!state.settings.remote_images);
    let bogus = h
        .write(|tx| tx.read().settings().get(AccountId::NONE, "bogus"))
        .await;
    assert_eq!(bogus, None, "an unknown setting is refused");
}

#[tokio::test]
async fn an_action_at_or_below_the_last_applied_is_dropped() {
    let h = Harness::start().await;
    let last = h.app.actions().last_received().0;
    let numbered = |n: i64, name: &str| NumberedAction {
        n: ActionNo(n),
        action: subscribe(name, SubKey::App),
    };
    h.app.actions().submit(vec![numbered(last + 1, "first")]);
    h.app
        .actions()
        .submit(vec![numbered(last + 1, "again"), numbered(last, "older")]);
    h.app.actions().drained().await.unwrap();
    let opened: Vec<String> = h
        .take()
        .into_iter()
        .flat_map(|m| m.updates)
        .map(|u| match u {
            SubUpdate::App { sub, .. } => sub.0,
            other => panic!("only app loads: {other:?}"),
        })
        .collect();
    assert_eq!(opened, ["first"]);
}

#[tokio::test]
async fn setting_values_and_accounts_are_checked() {
    let mut h = Harness::start().await;
    let (account, _) = h.write(seed).await;
    h.act(vec![
        Action::SetSetting {
            account: Some(account),
            key: "muted".to_owned(),
            value: "yes".to_owned(),
        },
        Action::SetSetting {
            account: Some(AccountId(99)),
            key: "muted".to_owned(),
            value: "on".to_owned(),
        },
        Action::SetSetting {
            account: None,
            key: "muted".to_owned(),
            value: "on".to_owned(),
        },
    ])
    .await;
    let stored = h
        .write(move |tx| {
            Ok((
                tx.read().settings().all(account)?,
                tx.read().settings().all(AccountId(99))?,
                tx.read().settings().all(AccountId::NONE)?,
            ))
        })
        .await;
    let none = std::collections::BTreeMap::new();
    assert_eq!(stored, (none.clone(), none.clone(), none));
}

#[tokio::test]
async fn muting_an_account_takes_its_unread_mail_off_the_badge() {
    let mut h = Harness::start().await;
    let (account, _) = h.write(seed).await;
    // The fixture's unread Inbox threads: every third of the first 25.
    let unread = 9;
    let badge = |h: &Harness| h.platform.badges.lock().last().copied().flatten();
    eventually("the badge counts the inbox", || badge(&h) == Some(unread)).await;
    h.act(vec![
        subscribe("app", SubKey::App),
        Action::SetSetting {
            account: Some(account),
            key: "muted".to_owned(),
            value: "on".to_owned(),
        },
    ])
    .await;
    eventually("the badge clears", || {
        h.platform.badges.lock().last() == Some(&None)
    })
    .await;
    let state = app_state(&h.take()).unwrap();
    assert!(state.accounts[0].muted, "the rail shows the mute");
}

#[tokio::test]
async fn an_acked_notice_leaves_the_summary() {
    let mut h = Harness::start().await;
    let (account, _) = h.write(seed).await;
    let notice = h
        .write(move |tx| {
            tx.notices().add(&NewNotice {
                account,
                kind: NoticeKind::Rejected,
                message: "Gmail refused it".to_owned(),
                draft: None,
                thread: None,
                created_at: Millis(1),
            })
        })
        .await;
    h.act(vec![subscribe("acc", SubKey::Account { account })])
        .await;
    assert_eq!(summary(&h.take()).unwrap().notices.len(), 1);
    h.act(vec![Action::AckNotice { notice }]).await;
    assert_eq!(summary(&h.take()).unwrap().notices, []);
}

#[tokio::test]
async fn closing_a_finished_sends_toast_drops_the_row_and_its_draft() {
    let mut h = Harness::start().await;
    let (account, _) = h.write(seed).await;
    let draft = DraftId::from("d1");
    let d = draft.clone();
    let outbox = h
        .write(move |tx| {
            let mut new = Draft::new(d.clone(), account, DraftKind::New, Millis(1));
            new.to = vec![Addr::new("b@example.com")];
            tx.drafts().put(&new)?;
            let id = tx.outbox().insert(&NewOutbox {
                account,
                draft: d,
                thread: None,
                send_after: Millis(2),
            })?;
            let mut row = tx.read().outbox().get(id)?.unwrap().outbox;
            row.state = OutboxState::Sent;
            tx.outbox().update(&row)?;
            Ok(id)
        })
        .await;
    h.act(vec![subscribe("acc", SubKey::Account { account })])
        .await;
    assert_eq!(summary(&h.take()).unwrap().outbox.len(), 1);
    h.act(vec![Action::CloseToast { outbox }]).await;
    assert_eq!(summary(&h.take()).unwrap().outbox, []);
    let gone = h
        .write(move |tx| tx.read().drafts().get(&draft).map(|d| d.is_none()))
        .await;
    assert!(gone, "a sent draft goes with its finished row");
}

#[tokio::test]
async fn going_offline_patches_the_app_with_nothing_written() {
    let mut h = Harness::start().await;
    h.act(vec![subscribe("app", SubKey::App)]).await;
    assert!(app_state(&h.take()).unwrap().online);
    h.app.lifecycle().set_online(false).await.unwrap();
    assert!(!app_state(&h.take()).unwrap().online);
    h.app.lifecycle().set_online(false).await.unwrap();
    assert_eq!(h.take(), [], "no change, no message");
}

#[tokio::test]
async fn queries_read_rows_bodies_and_contacts() {
    let h = Harness::start().await;
    let (account, threads) = h.write(seed).await;
    let first = threads[0];
    let message = h
        .write(move |tx| {
            let m = tx.read().messages().for_thread(first)?.remove(0).id;
            tx.messages().store_body(
                m,
                &NewBody {
                    text: Some("hi <there>".to_owned()),
                    is_plain: true,
                    ..NewBody::default()
                },
            )?;
            Ok(m)
        })
        .await;
    let q = h.app.queries();
    let rows = q
        .fetch_rows(false, vec![threads[1].to_string(), "nope".to_owned()])
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].thread, Some(threads[1]));
    let bodies = q
        .get_bodies(vec![message, MessageId(999_999)])
        .await
        .unwrap();
    assert_eq!(bodies.len(), 1, "no stored body, no entry");
    assert!(
        bodies[0].html.contains("&lt;there&gt;"),
        "{}",
        bodies[0].html
    );
    let sender = q.sender_threads(account, threads[0]).await.unwrap();
    assert_eq!(sender, [threads[0]]);
    assert_eq!(q.autocomplete(account, " ".to_owned()).await.unwrap(), []);
}

#[tokio::test]
async fn autocomplete_ranks_sent_to_above_received_from_and_skips_my_own() {
    let h = Harness::start().await;
    let (account, _) = h.write(seed).await;
    h.write(move |tx| {
        let seen = |email: &str, name: &str, sent, received| ub_store::Sighting {
            email: email.to_owned(),
            name: name.to_owned(),
            sent,
            received,
            on_list: false,
            at: Millis(1_700_000_000_000),
        };
        tx.contacts().count(
            account,
            &"c1".into(),
            &[
                seen("ann.received@example.org", "Ann Received", 0, 5),
                seen("ann.sent@example.org", "Ann Sent", 3, 0),
                seen(ACCOUNT_EMAIL, "Ann Me", 9, 9),
                seen("bob@example.org", "Bob", 9, 9),
            ],
        )
    })
    .await;
    let got: Vec<String> = h
        .app
        .queries()
        .autocomplete(account, "ann".to_owned())
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.email)
        .collect();
    assert_eq!(got, ["ann.sent@example.org", "ann.received@example.org"]);
}

#[tokio::test]
async fn quit_commits_queued_ui_actions_then_stops_everything() {
    let mut h = Harness::start().await;
    let (account, _) = h.write(seed).await;
    h.app.actions().submit(vec![NumberedAction {
        n: ActionNo(h.app.actions().last_received().0 + 1),
        action: Action::SetSetting {
            account: Some(account),
            key: "muted".to_owned(),
            value: "on".to_owned(),
        },
    }]);
    tokio::time::timeout(Duration::from_secs(10), h.app.lifecycle().quit())
        .await
        .expect("quit returns within its deadline");
    assert!(h.activity.is_idle(), "{:?}", h.activity.snapshot());
    let after = h
        .app
        .writer()
        .call(Priority::Ui, |_, _| Ok::<_, StoreError>(()))
        .await;
    assert!(matches!(after, Err(StoreError::WriterStopped)));
    // A new core on the same file sees the action committed.
    let dir = std::mem::replace(&mut h.dir, tempfile::tempdir().unwrap());
    let again = Harness::start_in(dir).await;
    let muted = again
        .write(move |tx| tx.read().settings().get(account, "muted"))
        .await;
    assert_eq!(muted.as_deref(), Some("on"));
}

#[tokio::test]
async fn snooze_choices_are_worked_out_at_app_time_in_the_local_zone() {
    use chrono::TimeZone as _;

    let h = Harness::start().await;
    let pool = std::sync::Arc::new(
        ub_store::ReadPool::open(
            &h.dir.path().join(crate::Config::DB_FILE),
            std::num::NonZeroUsize::MIN,
        )
        .unwrap(),
    );
    // A Monday morning, so every fixed choice is still ahead.
    let monday = chrono::Local
        .with_ymd_and_hms(2026, 11, 2, 9, 0, 0)
        .earliest()
        .unwrap();
    let clock = std::sync::Arc::new(crate::clock::ManualClock::new(
        std::sync::Arc::clone(&h.activity),
        Millis(monday.timestamp_millis()),
    ));
    let q = crate::Queries::new(pool, clock);
    let all = q.snooze_options("");
    let names: Vec<&str> = all
        .iter()
        .map(|c| c.label.split(" · ").next().unwrap())
        .collect();
    assert_eq!(names, ["Later today", "Tomorrow morning", "Next week"]);
    assert!(all.iter().all(|c| c.at.0 > monday.timestamp_millis()));
    let typed = q.snooze_options("tomorrow");
    assert_eq!(
        typed.len(),
        2,
        "the typed time, then the matching choice: {typed:?}"
    );
}

#[tokio::test]
async fn a_conversation_lists_its_messages_and_marks_the_accounts_own() {
    let mut h = Harness::start().await;
    let (_, threads) = h.write(seed).await;
    // Thread 1: a message from someone, then the account's reply.
    h.act(vec![subscribe(
        "c",
        SubKey::Conversation {
            thread: threads[1],
            trash_mode: false,
            open: true,
        },
    )])
    .await;
    let data = h
        .take()
        .into_iter()
        .flat_map(|m| m.updates)
        .find_map(|u| match u {
            SubUpdate::Conversation {
                first: true, data, ..
            } => Some(data),
            _ => None,
        })
        .unwrap();
    assert!(!data.gone);
    let from: Vec<(String, bool)> = data
        .messages
        .iter()
        .map(|m| (m.from.email.clone(), m.from_me))
        .collect();
    assert_eq!(
        from,
        [
            ("person1@example.org".to_owned(), false),
            (ACCOUNT_EMAIL.to_owned(), true)
        ]
    );
}

#[tokio::test]
async fn a_save_that_arrives_after_the_send_is_dropped() {
    let mut h = Harness::start().await;
    let (account, _) = h.write(seed).await;
    let draft = DraftId::from("d1");
    let save = |body: &str| Action::SaveDraft {
        draft: draft.clone(),
        save: DraftSave::Update {
            fields: DraftFields {
                from: None,
                to: vec![ub_ipc::Addr {
                    name: None,
                    email: "b@example.com".to_owned(),
                }],
                cc: Vec::new(),
                bcc: Vec::new(),
                subject: "Hi".to_owned(),
                body_html: body.to_owned(),
                attachments: Vec::new(),
            },
        },
    };
    h.act(vec![
        Action::SaveDraft {
            draft: draft.clone(),
            save: DraftSave::Create {
                account,
                kind: DraftKind::New,
                reply_all: false,
                thread: None,
                target: None,
                started_on_latest: false,
            },
        },
        save("<p>final</p>"),
        Action::SendDraft {
            draft: draft.clone(),
        },
    ])
    .await;
    h.act(vec![save("<p>late</p>")]).await;
    let d = draft.clone();
    let body = h
        .write(move |tx| {
            Ok(tx
                .read()
                .drafts()
                .get(&d)?
                .map(|r| (r.in_outbox, r.draft.body_html)))
        })
        .await;
    assert_eq!(
        body,
        Some((true, "<p>final</p>".to_owned())),
        "the outbox sends what was sent"
    );
}
