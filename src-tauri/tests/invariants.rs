use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;

use chrono::{TimeZone as _, Timelike as _, Utc};
use chrono_tz::{America::New_York, Europe::Berlin};
use rusqlite::Connection;
use serde_json::{Value, json};
use supermonkey_lib::domain::query::{ListRequirement, MatchTarget, Query, Scope, gmail_query};
use supermonkey_lib::domain::snooze::{parse_snooze_text, snooze_options, snooze_picker};
use supermonkey_lib::domain::{
    AccountId, Addr, GmailMessageId, GmailThreadId, IMPORTANT_POSITION, Identity, LabelId,
    MessageFacts, MessageId, Millis, OTHER_POSITION, Pin, Router, SendAsAddress, Split, SplitId,
    SplitKind, SystemLabel, Thread, ThreadId,
};
use supermonkey_lib::ipc::{Command, Submitted};
use supermonkey_lib::store::{
    Db, MessageBodyWrite, NewAttachment, NewMessage, ReadPool, StoreError, with_write_tx,
};

struct OracleMsg {
    gmail_id: &'static str,
    date_ms: i64,
    from: Addr,
    to: Vec<Addr>,
    cc: Vec<Addr>,
    subject: &'static str,
    snippet: &'static str,
    body: &'static str,
    labels: Vec<LabelId>,
    is_list: bool,
    is_draft: bool,
    has_att: bool,
}

type OracleSeed = (
    AccountId,
    Identity,
    HashMap<LabelId, String>,
    Vec<(ThreadId, Vec<(MessageFacts, String)>)>,
);

#[expect(
    clippy::too_many_lines,
    reason = "seeds a multi-thread fixture covering live, list, trashed, attachment, and label cases for the differential search oracle"
)]
fn seed_oracle_db(path: &std::path::Path) -> Result<OracleSeed, StoreError> {
    let custom_label = LabelId("L_FIN".to_owned());
    let mut user_labels = HashMap::new();
    user_labels.insert(custom_label.clone(), "Finance".to_owned());

    let specs: Vec<(&'static str, Vec<OracleMsg>)> = vec![
        (
            "gt-1",
            vec![OracleMsg {
                gmail_id: "gm-1",
                date_ms: 1_760_000_000_000,
                from: Addr::named("Carol Stone", "carol@other.test"),
                to: vec![Addr::new("ana@acme.test")],
                cc: Vec::new(),
                subject: "Quarterly invoice",
                snippet: "Brilliant ideas should be shared",
                body: "Brilliant ideas should be shared in Q4.",
                labels: vec![
                    SystemLabel::Inbox.label_id(),
                    SystemLabel::Unread.label_id(),
                ],
                is_list: false,
                is_draft: false,
                has_att: true,
            }],
        ),
        (
            "gt-2",
            vec![OracleMsg {
                gmail_id: "gm-2",
                date_ms: 1_761_000_000_000,
                from: Addr::named("Carol Acme", "carol@acme.test"),
                to: vec![Addr::new("ana@acme.test")],
                cc: vec![Addr::new("bob@corp.test")],
                subject: "Be brilliant today",
                snippet: "You must be brilliant today",
                body: "You must be brilliant today with invoice details.",
                labels: vec![
                    SystemLabel::Inbox.label_id(),
                    SystemLabel::Starred.label_id(),
                    custom_label.clone(),
                ],
                is_list: false,
                is_draft: false,
                has_att: false,
            }],
        ),
        (
            "gt-3",
            vec![
                OracleMsg {
                    gmail_id: "gm-3a",
                    date_ms: 1_761_500_000_000,
                    from: Addr::named("Daily Digest", "news@list.test"),
                    to: vec![Addr::new("ana@acme.test")],
                    cc: Vec::new(),
                    subject: "Weekly digest",
                    snippet: "Digest of invoice trends",
                    body: "Digest of invoice trends and updates.",
                    labels: vec![SystemLabel::Inbox.label_id()],
                    is_list: true,
                    is_draft: false,
                    has_att: false,
                },
                OracleMsg {
                    gmail_id: "gm-3b",
                    date_ms: 1_761_600_000_000,
                    from: Addr::named("Ana", "ana@acme.test"),
                    to: vec![Addr::new("news@list.test")],
                    cc: Vec::new(),
                    subject: "Re: Weekly digest",
                    snippet: "Noted thanks",
                    body: "Noted thanks.",
                    labels: vec![SystemLabel::Sent.label_id()],
                    is_list: false,
                    is_draft: false,
                    has_att: false,
                },
            ],
        ),
        (
            "gt-4",
            vec![OracleMsg {
                gmail_id: "gm-4",
                date_ms: 1_762_000_000_000,
                from: Addr::named("Trash Sender", "trash@x.test"),
                to: vec![Addr::new("ana@acme.test")],
                cc: Vec::new(),
                subject: "Old trashed invoice",
                snippet: "Hidden in trash",
                body: "Hidden in trash.",
                labels: vec![SystemLabel::Trash.label_id()],
                is_list: true,
                is_draft: false,
                has_att: false,
            }],
        ),
        (
            "gt-5",
            vec![
                OracleMsg {
                    gmail_id: "gm-5a",
                    date_ms: 1_762_100_000_000,
                    from: Addr::named("Pat", "pat@corp.test"),
                    to: vec![Addr::new("ana@acme.test")],
                    cc: Vec::new(),
                    subject: "Mixed live and trashed list",
                    snippet: "Normal personal note",
                    body: "Normal personal note about digest.",
                    labels: vec![SystemLabel::Inbox.label_id()],
                    is_list: false,
                    is_draft: false,
                    has_att: false,
                },
                OracleMsg {
                    gmail_id: "gm-5b",
                    date_ms: 1_762_200_000_000,
                    from: Addr::named("Spam List", "bot@list.test"),
                    to: vec![Addr::new("ana@acme.test")],
                    cc: Vec::new(),
                    subject: "Mixed live and trashed list",
                    snippet: "Trashed list copy",
                    body: "Trashed list copy.",
                    labels: vec![SystemLabel::Trash.label_id()],
                    is_list: true,
                    is_draft: false,
                    has_att: false,
                },
            ],
        ),
    ];

    let mut inserted = Vec::new();
    let acct_id = with_write_tx(path, |tx| {
        let acct = tx.create_account("ana@acme.test", "Ana")?;
        tx.create_local_label(acct.id, &custom_label, "Finance")?;
        for (gtid, msgs) in specs {
            let tid = tx.place_thread(acct.id, &GmailThreadId(gtid.to_owned()), None)?;
            let mut thread_msgs = Vec::new();
            for m in msgs {
                let mid = tx.upsert_message(
                    acct.id,
                    tid,
                    &NewMessage {
                        gmail_id: GmailMessageId(m.gmail_id.to_owned()),
                        date: Millis(m.date_ms),
                        from: m.from.clone(),
                        to: m.to.clone(),
                        cc: m.cc.clone(),
                        bcc: Vec::new(),
                        reply_to: Vec::new(),
                        subject: m.subject.to_owned(),
                        snippet: m.snippet.to_owned(),
                        message_id_hdr: None,
                        in_reply_to: None,
                        references: Vec::new(),
                        is_list: m.is_list,
                        is_draft: m.is_draft,
                        sm_draft_id: None,
                        labels: m.labels.iter().cloned().collect(),
                        has_attachments: Some(m.has_att),
                    },
                )?;
                let atts = if m.has_att {
                    vec![NewAttachment {
                        part_id: "1".to_owned(),
                        gmail_att_id: Some(supermonkey_lib::domain::GmailAttachmentId(
                            "a1".to_owned(),
                        )),
                        filename: "doc.pdf".to_owned(),
                        mime_type: "application/pdf".to_owned(),
                        size: 12,
                        content_id: None,
                        is_inline: false,
                        local_path: None,
                    }]
                } else {
                    Vec::new()
                };
                tx.store_message_body(
                    mid,
                    &MessageBodyWrite {
                        html: m.body,
                        quote_html: None,
                        body_text: m.body,
                        attachments: &atts,
                    },
                )?;
                thread_msgs.push((
                    MessageFacts {
                        id: mid,
                        gmail_id: GmailMessageId(m.gmail_id.to_owned()),
                        date: Millis(m.date_ms),
                        from: m.from,
                        to: m.to,
                        cc: m.cc,
                        bcc: Vec::new(),
                        reply_to: Vec::new(),
                        subject: m.subject.to_owned(),
                        snippet: m.snippet.to_owned(),
                        message_id_hdr: None,
                        in_reply_to: None,
                        references: Vec::new(),
                        is_list: m.is_list,
                        is_draft: m.is_draft,
                        has_attachments: m.has_att,
                        labels: m.labels.into_iter().collect(),
                    },
                    m.body.to_owned(),
                ));
            }
            tx.derive_thread(tid)?;
            inserted.push((tid, thread_msgs));
        }
        Ok::<_, StoreError>(acct.id)
    })?;

    let identity = Identity::new("ana@acme.test", "Ana", Vec::new());
    Ok((acct_id, identity, user_labels, inserted))
}

fn oracle_matches(
    query: &Query,
    identity: &Identity,
    labels: &HashMap<LabelId, String>,
    threads: &[(ThreadId, Vec<(MessageFacts, String)>)],
) -> HashSet<ThreadId> {
    let scope = query.scope();
    let mut out = HashSet::new();
    for (tid, msgs) in threads {
        let hit = msgs.iter().any(|(m, body)| {
            let in_scope = match scope {
                Scope::Normal => m.is_live(),
                Scope::Trash => !m.is_draft && m.has(SystemLabel::Trash),
                Scope::Spam => !m.is_draft && m.has(SystemLabel::Spam),
                Scope::Anywhere => !m.is_draft,
                Scope::Drafts => m.is_draft,
            };
            in_scope && query.matches(&MatchTarget::search(m, body, labels, identity, false))
        });
        if hit {
            out.insert(*tid);
        }
    }
    out
}

#[test]
fn search_sql_and_fts5_match_naive_oracle() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("oracle.db");
    let (acct, identity, labels, threads) = seed_oracle_db(&db_path).unwrap();
    let pool = ReadPool::open(&db_path, NonZeroUsize::new(2).unwrap()).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 11, 2, 15, 0, 0).unwrap();

    let queries = [
        "invoice",
        "from:carol@acme.test",
        "from:carol@other.test",
        "\"be brilliant\"",
        "\"brilliant ideas\"",
        "to:ana@acme.test cc:bob@corp.test",
        "from:carol@acme.test OR from:pat@corp.test",
        "from:carol@other.test has:attachment OR is:starred",
        "digest is:list",
        "digest -is:list",
        "is:list -is:list digest",
        "is:unread",
        "is:read invoice",
        "is:starred",
        "has:attachment",
        "label:Finance",
        "in:sent",
        "in:trash",
        "in:anywhere invoice",
        "invoice -brilliant",
        "invoice - trash",
    ];

    for raw in queries {
        let q = Query::parse_search(raw, &now);
        let actual: HashSet<ThreadId> = pool
            .with(|db: Db<'_>| db.search_threads(acct, &q, &[]))
            .unwrap()
            .into_iter()
            .collect();
        let expected = oracle_matches(&q, &identity, &labels, &threads);
        assert_eq!(
            actual, expected,
            "SQL/FTS5 search diverged from oracle for {raw:?}"
        );
    }
}

const QUERY_FUZZ_PIECES: &[&str] = &[
    "from:",
    "to:",
    "cc:",
    "subject:",
    "label:",
    "in:",
    "is:list",
    "-is:list",
    "older_than:",
    "newer_than:",
    "after:",
    "before:",
    "OR",
    "AND",
    "-",
    "--",
    "(",
    ")",
    "\"",
    "*",
    ":",
    " ",
    "1年",
    "2д",
    "3é",
    "café",
    "a@b.c",
    "🚀",
    "\0",
    "foo",
    "bar",
];

#[test]
fn query_parsers_and_fts5_never_panic_on_seeded_unicode_fuzz() {
    let now = Utc.with_ymd_and_hms(2026, 11, 2, 15, 0, 0).unwrap();
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE VIRTUAL TABLE t USING fts5(from_text, to_text, cc_text, subject, body, tokenize='unicode61 remove_diacritics 2');",
    )
    .unwrap();
    let mut seed: u64 = 0xD1CE_5EED_1234_5678;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };

    for _ in 0..3_000 {
        let count = usize::try_from(next() % 10).unwrap_or(0);
        let mut raw = String::new();
        for _ in 0..count {
            let idx = usize::try_from(next() % u64::try_from(QUERY_FUZZ_PIECES.len()).unwrap_or(1))
                .unwrap_or(0);
            raw.push_str(QUERY_FUZZ_PIECES[idx]);
        }
        let q = Query::parse_search(&raw, &now);
        let _rule = Query::parse_rule(&raw);
        let _gmail = gmail_query(&raw);
        if let Some(fts) = q.to_fts5() {
            let mut stmt = conn
                .prepare("SELECT rowid FROM t WHERE t MATCH ?1")
                .unwrap();
            let res: Result<Vec<i64>, _> = stmt.query_map([&fts], |r| r.get(0)).unwrap().collect();
            assert!(res.is_ok(), "FTS5 rejected {fts:?} produced from {raw:?}");
        }
    }
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "verifies every split-rule operator family end-to-end through Query::parse_rule and Router::route"
)]
fn split_rule_operators_accepted_by_parser_are_evaluated_by_router() {
    let identity = Identity::new(
        "ana@acme.test",
        "Ana",
        vec![SendAsAddress {
            email: "alias@acme.test".to_owned(),
            display_name: "Ana Alias".to_owned(),
            primary: false,
        }],
    );
    let custom_lbl = LabelId("L_VIP".to_owned());
    let labels = HashMap::from([(custom_lbl.clone(), "VIP".to_owned())]);

    let make_msg = |from: &str,
                    to: &str,
                    cc: &str,
                    subj: &str,
                    is_list: bool,
                    has_att: bool,
                    lbls: Vec<LabelId>| {
        MessageFacts {
            id: MessageId(1),
            gmail_id: GmailMessageId("gm1".to_owned()),
            date: Millis(1_000),
            from: Addr::new(from),
            to: vec![Addr::new(to)],
            cc: if cc.is_empty() {
                Vec::new()
            } else {
                vec![Addr::new(cc)]
            },
            bcc: Vec::new(),
            reply_to: Vec::new(),
            subject: subj.to_owned(),
            snippet: String::new(),
            message_id_hdr: None,
            in_reply_to: None,
            references: Vec::new(),
            is_list,
            is_draft: false,
            has_attachments: has_att,
            labels: lbls.into_iter().collect(),
        }
    };

    let cases = [
        (
            "from:bob@x.test",
            make_msg(
                "bob@x.test",
                "ana@acme.test",
                "",
                "Hi",
                false,
                false,
                vec![],
            ),
            make_msg(
                "eve@x.test",
                "ana@acme.test",
                "",
                "Hi",
                false,
                false,
                vec![],
            ),
        ),
        (
            "to:me",
            make_msg(
                "bob@x.test",
                "alias@acme.test",
                "",
                "Hi",
                false,
                false,
                vec![],
            ),
            make_msg("bob@x.test", "other@x.test", "", "Hi", false, false, vec![]),
        ),
        (
            "cc:me",
            make_msg(
                "bob@x.test",
                "other@x.test",
                "ana@acme.test",
                "Hi",
                false,
                false,
                vec![],
            ),
            make_msg(
                "bob@x.test",
                "ana@acme.test",
                "",
                "Hi",
                false,
                false,
                vec![],
            ),
        ),
        (
            "from:me",
            make_msg(
                "ana@acme.test",
                "bob@x.test",
                "",
                "Hi",
                false,
                false,
                vec![],
            ),
            make_msg(
                "bob@x.test",
                "ana@acme.test",
                "",
                "Hi",
                false,
                false,
                vec![],
            ),
        ),
        (
            "subject:budget",
            make_msg(
                "bob@x.test",
                "ana@acme.test",
                "",
                "Q4 budget",
                false,
                false,
                vec![],
            ),
            make_msg(
                "bob@x.test",
                "ana@acme.test",
                "",
                "Lunch",
                false,
                false,
                vec![],
            ),
        ),
        (
            "label:VIP",
            make_msg(
                "bob@x.test",
                "ana@acme.test",
                "",
                "Hi",
                false,
                false,
                vec![custom_lbl],
            ),
            make_msg(
                "bob@x.test",
                "ana@acme.test",
                "",
                "Hi",
                false,
                false,
                vec![],
            ),
        ),
        (
            "is:list",
            make_msg("bob@x.test", "ana@acme.test", "", "Hi", true, false, vec![]),
            make_msg(
                "bob@x.test",
                "ana@acme.test",
                "",
                "Hi",
                false,
                false,
                vec![],
            ),
        ),
        (
            "(from:bob@x.test OR subject:budget)",
            make_msg(
                "eve@x.test",
                "ana@acme.test",
                "",
                "Q4 budget",
                false,
                false,
                vec![],
            ),
            make_msg(
                "eve@x.test",
                "ana@acme.test",
                "",
                "Lunch",
                false,
                false,
                vec![],
            ),
        ),
    ];

    for rejected in [
        "has:attachment",
        "is:unread",
        "in:sent",
        "category:promotions",
        "older_than:7d",
    ] {
        assert!(
            Query::parse_rule(rejected).is_err(),
            "search-only operator {rejected:?} must be rejected by split rule parser"
        );
    }

    for (rule, yes_msg, no_msg) in cases {
        assert!(Query::parse_rule(rule).is_ok(), "rule {rule:?} must parse");
        let router = Router::new(
            vec![
                Split {
                    id: SplitId(10),
                    name: "Rule".to_owned(),
                    kind: SplitKind::Custom,
                    position: 0,
                    query: rule.to_owned(),
                    enabled: true,
                    hide_when_empty: false,
                },
                Split {
                    id: SplitId(1),
                    name: "Important".to_owned(),
                    kind: SplitKind::Important,
                    position: IMPORTANT_POSITION,
                    query: String::new(),
                    enabled: true,
                    hide_when_empty: false,
                },
                Split {
                    id: SplitId(2),
                    name: "Other".to_owned(),
                    kind: SplitKind::Other,
                    position: OTHER_POSITION,
                    query: String::new(),
                    enabled: true,
                    hide_when_empty: false,
                },
            ],
            Vec::<Pin>::new(),
        );
        assert_eq!(
            router.route(&Thread::new(vec![yes_msg]), &identity, &labels),
            Some(SplitId(10)),
            "matching thread must route to custom split for {rule:?}"
        );
        assert_ne!(
            router.route(&Thread::new(vec![no_msg]), &identity, &labels),
            Some(SplitId(10)),
            "non-matching thread must not route to custom split for {rule:?}"
        );
    }
}

const SNOOZE_FUZZ_TOKENS: &[&str] = &[
    "in",
    " ",
    "2",
    "999999999",
    "days",
    "weeks",
    "hours",
    "mins",
    "tomorrow",
    "mon",
    "jan",
    "31",
    "2:30am",
    "25:99",
    "年",
    "é",
    "-1",
    "",
];

fn assert_snooze_fuzz_invariants(anchor: &chrono::DateTime<chrono_tz::Tz>) {
    let mut seed: u64 = 0x5002_E000_CAFE_0001;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for _ in 0..1_000 {
        let n = usize::try_from(next() % 6).unwrap_or(0);
        let mut s = String::new();
        for _ in 0..n {
            let idx =
                usize::try_from(next() % u64::try_from(SNOOZE_FUZZ_TOKENS.len()).unwrap_or(1))
                    .unwrap_or(0);
            s.push_str(SNOOZE_FUZZ_TOKENS[idx]);
        }
        if let Some(dt) = parse_snooze_text(&s, anchor) {
            assert!(
                dt > *anchor,
                "parsed snooze {s:?} must be strictly in the future"
            );
        }
        let _picks = snooze_picker(&s, anchor);
    }
}

#[test]
fn snooze_parser_and_picker_hold_invariants_across_dst_and_fuzz() {
    let anchors_ny = [
        New_York.with_ymd_and_hms(2026, 3, 7, 14, 30, 0).unwrap(),
        New_York.with_ymd_and_hms(2026, 10, 31, 14, 30, 0).unwrap(),
    ];
    for now in anchors_ny {
        let now_ms = now.timestamp_millis();
        let opts = snooze_options(&now);
        assert!(!opts.is_empty(), "snooze_options returns preset choices");
        for opt in &opts {
            assert!(
                opt.at.timestamp_millis() > now_ms,
                "preset must be in the future"
            );
        }
        let in_two_days = parse_snooze_text("in 2 days", &now).expect("parses in 2 days");
        assert_eq!(
            in_two_days.hour(),
            14,
            "wall-clock hour preserved across NY DST"
        );
        assert_eq!(
            in_two_days.minute(),
            30,
            "wall-clock minute preserved across NY DST"
        );
        let picks = snooze_picker("tomorrow 9am", &now);
        assert!(
            !picks.is_empty(),
            "picker matches tomorrow 9am across NY DST"
        );
        assert!(
            picks[0].at.0 > now_ms,
            "picker result is strictly after now"
        );
    }

    let berlin_spring = Berlin.with_ymd_and_hms(2026, 3, 28, 10, 0, 0).unwrap();
    let berlin_next =
        parse_snooze_text("tomorrow 2:30am", &berlin_spring).expect("handles Berlin gap");
    assert!(
        berlin_next > berlin_spring,
        "gap-shifted Berlin time is after now"
    );

    assert_snooze_fuzz_invariants(&anchors_ny[0]);
}

#[test]
fn ipc_command_and_submitted_handle_adversarial_json_without_panicking_or_leaking() {
    let secret = "PRIVATE_USER_BODY_SECRET_999";
    let bad_actions = [
        json!({"n": 1, "action": {"type": "modify", "account": "wrong", "note": secret}}),
        json!({"n": 2, "action": {"type": "unknownAction", "body": secret}}),
        json!({"n": 3, "action": secret}),
        json!({"n": 4, "action": null}),
        json!({"n": 5, "action": [1, 2, 3]}),
    ];
    for raw in bad_actions {
        let sub: Submitted =
            serde_json::from_value(raw).expect("Submitted absorbs unreadable actions");
        let err = sub
            .action
            .expect_err("malformed action must deserialize as Err(Unreadable)");
        let rendered = format!("{err:?}");
        assert!(
            !rendered.contains(secret),
            "Unreadable debug output must never leak user text: {rendered}"
        );
    }

    let bad_commands: &[(&str, Value)] = &[
        ("submit", json!({"actions": "not-an-array"})),
        ("nonexistent", json!({})),
        ("fetch_rows", json!(null)),
        ("open_url", json!({"url": 12345})),
        ("snooze_options", json!({"text": ["bad"]})),
    ];
    for (name, args) in bad_commands {
        assert!(
            Command::parse(name, args.clone()).is_err(),
            "malformed command {name} must return Err"
        );
    }
}

const LIST_REQ_PIECES: &[&str] = &[
    "is:list",
    "wombat",
    "from:bob@x.test",
    "OR",
    "AND",
    "-",
    "(",
    ")",
    " ",
];

fn list_req_probe_message(is_list: bool, has_wombat: bool, from_bob: bool) -> MessageFacts {
    MessageFacts {
        id: MessageId(1),
        gmail_id: GmailMessageId("gm-list".to_owned()),
        date: Millis(1_000),
        from: Addr::new(if from_bob { "bob@x.test" } else { "eve@x.test" }),
        to: vec![Addr::new("ana@acme.test")],
        cc: Vec::new(),
        bcc: Vec::new(),
        reply_to: Vec::new(),
        subject: if has_wombat { "wombat" } else { "other" }.to_owned(),
        snippet: String::new(),
        message_id_hdr: None,
        in_reply_to: None,
        references: Vec::new(),
        is_list,
        is_draft: false,
        has_attachments: false,
        labels: std::iter::once(SystemLabel::Inbox.label_id()).collect(),
    }
}

fn assert_list_req_fuzz_soundness(now: chrono::DateTime<Utc>) {
    let identity = Identity::new("ana@acme.test", "Ana", Vec::new());
    let labels = HashMap::new();
    let mut seed: u64 = 0x1157_BEEF_9999_0001;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for _ in 0..1_500 {
        let len = usize::try_from(1 + next() % 8).unwrap_or(1);
        let mut raw = String::new();
        for _ in 0..len {
            let idx = usize::try_from(next() % u64::try_from(LIST_REQ_PIECES.len()).unwrap_or(1))
                .unwrap_or(0);
            raw.push_str(LIST_REQ_PIECES[idx]);
        }
        let q = Query::parse_search(&raw, &now);
        let req = q.list_requirement();
        for is_list in [false, true] {
            for has_wombat in [false, true] {
                for from_bob in [false, true] {
                    let msg = list_req_probe_message(is_list, has_wombat, from_bob);
                    let target = MatchTarget::search(&msg, "", &labels, &identity, false);
                    if !q.matches(&target) {
                        continue;
                    }
                    assert_ne!(
                        req,
                        ListRequirement::Impossible,
                        "query {raw:?} matched a message despite Impossible requirement"
                    );
                    assert!(
                        req != ListRequirement::OnlyList || is_list,
                        "query {raw:?} matched non-list mail despite OnlyList"
                    );
                    assert!(
                        req != ListRequirement::ExcludeList || !is_list,
                        "query {raw:?} matched list mail despite ExcludeList"
                    );
                }
            }
        }
    }
}

#[test]
fn list_requirement_is_sound_over_boolean_combinations_and_negated_groups() {
    let now = Utc.with_ymd_and_hms(2026, 11, 2, 15, 0, 0).unwrap();

    assert_eq!(
        Query::parse_search("-(is:list wombat)", &now).list_requirement(),
        ListRequirement::Any,
        "negated conjunction -(is:list wombat) can match list mail without wombat"
    );
    assert_eq!(
        Query::parse_search("-(is:list OR wombat)", &now).list_requirement(),
        ListRequirement::ExcludeList,
        "negated disjunction -(is:list OR wombat) excludes all list mail"
    );
    assert_eq!(
        Query::parse_search("is:list OR is:list", &now).list_requirement(),
        ListRequirement::OnlyList,
        "disjunction where every branch requires list mail requires list mail"
    );
    assert_eq!(
        Query::parse_search("-(is:list)", &now).list_requirement(),
        ListRequirement::ExcludeList,
        "negated singleton group -(is:list) excludes list mail"
    );
    assert_eq!(
        Query::parse_search("-(is:list -is:list)", &now).list_requirement(),
        ListRequirement::Any,
        "negated contradiction matches both list and non-list mail"
    );

    assert_list_req_fuzz_soundness(now);
}
