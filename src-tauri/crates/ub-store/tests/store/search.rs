//! Local search over the FTS5 index, with the query language's post-filter.

use ub_domain::query::FTS_COLUMNS;
use ub_domain::{AccountId, Addr, Query, ThreadId};
use ub_store::{GmailLabel, NewBody};

use crate::common::{Harness, fetched, msg};

/// Stores a one-message thread with a plain body.
fn mail(
    h: &Harness,
    account: AccountId,
    gmail: &'static str,
    from: &'static str,
    labels: &'static [&'static str],
    body: &'static str,
) -> ThreadId {
    let stored = h.write(move |tx| {
        let mut m = msg(&format!("{gmail}-m"), 1_000, from, labels);
        m.to = vec![Addr::named("Dana Q", "dana@example.com")];
        tx.threads().store(&fetched(account, gmail, vec![m]))
    });
    let message = stored.added[0];
    h.write(move |tx| {
        tx.messages().store_body(
            message,
            &NewBody {
                text: Some(body.to_owned()),
                is_plain: true,
                ..NewBody::default()
            },
        )
    });
    stored.thread.unwrap()
}

fn search(h: &Harness, account: AccountId, query: &str, hits: Vec<ThreadId>) -> Vec<ThreadId> {
    let query = Query::parse(query);
    h.read(move |r| r.search().threads(account, &query, &hits))
}

#[test]
fn the_index_has_the_columns_the_query_language_targets() {
    let h = Harness::start();
    let conn = rusqlite::Connection::open(h.path()).unwrap();
    let mut stmt = conn
        .prepare("SELECT name FROM pragma_table_info('messages_fts')")
        .unwrap();
    let columns: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(columns, FTS_COLUMNS);
}

#[test]
fn search_finds_headers_and_bodies() {
    let h = Harness::start();
    let account = h.account("me@example.com");
    let budget = mail(
        &h,
        account,
        "t1",
        "alice@example.com",
        &["INBOX"],
        "The Q3 budget is attached",
    );
    let lunch = mail(
        &h,
        account,
        "t2",
        "bob@example.com",
        &[],
        "Lunch on Friday?",
    );
    assert_eq!(search(&h, account, "budget", vec![]), [budget], "body text");
    assert_eq!(search(&h, account, "from:bob", vec![]), [lunch]);
    assert_eq!(
        search(&h, account, "to:dana", vec![]).len(),
        2,
        "recipient names"
    );
    assert_eq!(search(&h, account, "budget OR lunch", vec![]).len(), 2);
    assert!(
        search(&h, account, "nothing-like-this", vec![]).is_empty(),
        "no match"
    );
    // A body stored later is indexed too.
    assert!(search(&h, account, "friday", vec![]).contains(&lunch));
}

#[test]
fn label_terms_are_decided_by_the_post_filter() {
    let h = Harness::start();
    let account = h.account("me@example.com");
    h.write(move |tx| {
        tx.labels().sync_from_gmail(
            account,
            &[GmailLabel {
                gmail_id: "Label_1".into(),
                name: "Work".to_owned(),
                system: false,
            }],
        )
    });
    let work = mail(
        &h,
        account,
        "t1",
        "alice@example.com",
        &["Label_1"],
        "budget review",
    );
    mail(&h, account, "t2", "bob@example.com", &[], "budget party");
    assert_eq!(search(&h, account, "label:work budget", vec![]), [work]);
    assert_eq!(
        search(&h, account, "label:work", vec![]),
        [work],
        "no index term at all"
    );
}

#[test]
fn search_skips_trash_spam_and_drafts_but_lists_gmail_hits() {
    let h = Harness::start();
    let account = h.account("me@example.com");
    let trashed = mail(
        &h,
        account,
        "t1",
        "alice@example.com",
        &["TRASH"],
        "secret plan",
    );
    let mut draft = msg("t2-m", 1_000, "me@example.com", &["DRAFT"]);
    draft.is_draft = true;
    draft.subject = "secret draft".to_owned();
    h.write(move |tx| tx.threads().store(&fetched(account, "t2", vec![draft])));
    assert!(
        search(&h, account, "secret", vec![]).is_empty(),
        "index path"
    );
    assert!(
        search(&h, account, "secret label:inbox", vec![]).is_empty(),
        "post-filter too"
    );

    let other = mail(
        &h,
        account,
        "t3",
        "carol@example.com",
        &["INBOX"],
        "unrelated",
    );
    assert_eq!(
        search(&h, account, "secret", vec![other, trashed, ThreadId(404)]),
        [other],
        "a Gmail hit is listed while it has a live message"
    );
}

#[test]
fn a_trashed_message_does_not_match_for_its_live_thread() {
    let h = Harness::start();
    let account = h.account("me@example.com");
    let mut kept = msg("m1", 1_000, "alice@example.com", &["INBOX"]);
    "kept".clone_into(&mut kept.subject);
    let mut trashed = msg("m2", 2_000, "bob@example.com", &["TRASH"]);
    "secret".clone_into(&mut trashed.subject);
    trashed.is_list = true;
    let thread = h
        .write(move |tx| {
            tx.threads()
                .store(&fetched(account, "t1", vec![kept, trashed]))
        })
        .thread
        .unwrap();
    assert_eq!(search(&h, account, "kept", vec![]), [thread]);
    assert!(
        search(&h, account, "secret", vec![]).is_empty(),
        "index path"
    );
    // `is:list` has no index term: every live thread is checked one by one.
    assert!(
        search(&h, account, "is:list", vec![]).is_empty(),
        "post-filter path"
    );
}
