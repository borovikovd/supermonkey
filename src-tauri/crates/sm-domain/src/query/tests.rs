//! Cases ported from the old `sm-rules` query tests.

use super::*;
use crate::test_support::message;
use crate::{Addr, Identity, MessageFacts};

fn p(s: &str) -> String {
    Query::parse(s).to_string()
}

/// The clock the E2E seeds use: Monday 2026-11-02 10:00 in New York.
fn now() -> chrono::DateTime<chrono_tz::Tz> {
    use chrono::TimeZone;
    chrono_tz::America::New_York
        .with_ymd_and_hms(2026, 11, 2, 10, 0, 0)
        .unwrap()
}

/// Milliseconds at a New York wall time.
fn ny(y: i32, mo: u32, d: u32, h: u32) -> i64 {
    use chrono::TimeZone;
    chrono_tz::America::New_York
        .with_ymd_and_hms(y, mo, d, h, 0, 0)
        .unwrap()
        .timestamp_millis()
}

fn search(input: &str) -> Query {
    Query::parse_search(input, &now())
}

fn s(input: &str) -> String {
    search(input).to_string()
}

fn strict_err(s: &str) -> String {
    Query::parse_rule(s).unwrap_err().to_string()
}

// --- parsing --------------------------------------------------------

#[test]
fn empty_and_blank_input_is_the_empty_query() {
    assert_eq!(p(""), "(and)");
    assert_eq!(p("   \t "), "(and)");
    assert_eq!(strict_err(""), "Enter a query");
    assert_eq!(strict_err("   "), "Enter a query");
    assert!(Query::parse("  ").is_empty());
}

#[test]
fn operators_and_bare_words() {
    assert_eq!(p("from:alice"), r#"from:"alice"*"#);
    assert_eq!(p("from:alice "), r#"from:"alice""#);
    assert_eq!(
        p("to:bob cc:carol bcc:dan subject:hi label:Work is:list word "),
        r#"(and to:"bob" cc:"carol" bcc:"dan" subject:"hi" label:"Work" is:list "word")"#
    );
}

#[test]
fn operator_names_and_keywords_are_case_insensitive() {
    assert_eq!(p("FROM:Alice Is:LIST "), r#"(and from:"Alice" is:list)"#);
    assert_eq!(p("a or b "), r#"(or "a" "b")"#);
    assert_eq!(p("a and b "), r#"(and "a" "b")"#);
}

#[test]
fn or_with_quoted_operator_value() {
    assert_eq!(
        p(r#"from:alice OR from:bob subject:"q3 plan""#),
        r#"(or from:"alice" (and from:"bob" subject:"q3 plan"))"#
    );
}

#[test]
fn and_binds_tighter_than_or() {
    assert_eq!(p("a b OR c d "), r#"(or (and "a" "b") (and "c" "d"))"#);
    assert_eq!(p("a AND b OR c "), r#"(or (and "a" "b") "c")"#);
    assert_eq!(p("a OR b AND c "), r#"(or "a" (and "b" "c"))"#);
}

#[test]
fn parentheses_group_and_nest() {
    assert_eq!(p("(a OR b) c "), r#"(and (or "a" "b") "c")"#);
    assert_eq!(
        p("((a OR (b c)) OR d) e "),
        r#"(and (or "a" (and "b" "c") "d") "e")"#
    );
    assert_eq!(p("(((a))) "), r#""a""#);
    assert_eq!(p("x(y)z "), r#"(and "x" "y" "z")"#);
}

#[test]
fn quoted_phrases() {
    assert_eq!(p(r#""q3 plan""#), r#""q3 plan""#);
    assert_eq!(p(r#""q3 plan"x"#), r#"(and "q3 plan" "x"*)"#);
    // Unclosed: the user is still typing it.
    assert_eq!(p(r#""q3 pl"#), r#""q3 pl"*"#);
    assert_eq!(strict_err(r#""q3 pl"#), "Missing closing quote");
    assert_eq!(p(r#"subject:"q3 pl"#), r#"subject:"q3 pl"*"#);
    assert_eq!(strict_err(r#"subject:"q3"#), "Missing closing quote");
    // A phrase can hold keywords and parentheses.
    assert_eq!(p(r#""a OR (b)""#), r#""a OR (b)""#);
}

#[test]
fn unsupported_operators_are_kept() {
    assert_eq!(
        p("filename:pdf larger:1M has:drive is:muted x "),
        r#"(and ?filename:pdf ?larger:1M ?has:drive ?is:muted "x")"#
    );
    assert_eq!(p("from:(a b) c "), r#"(and ?from:(a b) "c")"#);
    assert_eq!(p("in:(a b) c "), r#"(and ?in:(a b) "c")"#);
    // Values that are no date are left to Gmail too.
    assert_eq!(
        s("after:yesterday before:2026/13/01 newer_than:2w older_than:d "),
        "(and ?after:yesterday ?before:2026/13/01 ?newer_than:2w ?older_than:d)"
    );
    // Without a clock, dates can't be placed in time.
    assert_eq!(p("after:2026/01/31 "), "?after:2026/01/31");
    assert_eq!(
        strict_err("filename:pdf"),
        "filename:pdf isn't supported in split rules"
    );
}

#[test]
fn search_operators() {
    assert_eq!(
        p("in:sent in:Inbox in:TRASH in:anywhere in:snoozed in:drafts in:work-projects "),
        r#"(and label:"sent" label:"Inbox" label:"TRASH" in:anywhere in:snoozed label:"DRAFT" label:"work-projects")"#
    );
    assert_eq!(
        p("is:unread is:read is:STARRED is:important is:snoozed is:list "),
        r#"(and label:"UNREAD" (not label:"UNREAD") label:"STARRED" label:"IMPORTANT" in:snoozed is:list)"#
    );
    assert_eq!(
        p("has:attachment category:primary category:Promotions "),
        r#"(and has:attachment label:"CATEGORY_PERSONAL" label:"CATEGORY_PROMOTIONS")"#
    );
    assert_eq!(p(r#"in:"Work Projects""#), r#"label:"Work Projects""#);
    // Still typing an operator: nothing to match on yet.
    assert_eq!(p("is:"), "(and)");
}

#[test]
fn negation() {
    assert_eq!(p("-foo bar "), r#"(and (not "foo") "bar")"#);
    assert_eq!(p("-in:inbox "), r#"(not label:"inbox")"#);
    assert_eq!(p(r#"-"a b" c "#), r#"(and (not "a b") "c")"#);
    assert_eq!(p("-(a OR b) c "), r#"(and (not (or "a" "b")) "c")"#);
    assert_eq!(p("--a "), r#"(not (not "a"))"#);
    assert_eq!(p("-OR "), r#"(not "OR")"#);
    assert_eq!(p("a OR -b "), r#"(or "a" (not "b"))"#);
    // A `-` alone, inside a word, or before `)` negates nothing.
    assert_eq!(p("a - b "), r#"(and "a" "b")"#);
    assert_eq!(p("e-mail "), r#""e-mail""#);
    assert_eq!(p("(a -) "), r#""a""#);
    assert_eq!(p("-& a "), r#""a""#);
}

#[test]
fn split_rules_refuse_search_operators() {
    for (rule, term) in [
        ("-foo", "-foo"),
        ("a -(b OR c)", "-(b OR c)"),
        ("in:sent", "in:sent"),
        ("is:unread", "is:unread"),
        ("is:read", "is:read"),
        ("has:attachment", "has:attachment"),
        ("category:social", "category:social"),
        ("from:a after:2026/01/31", "after:2026/01/31"),
        ("newer_than:2d", "newer_than:2d"),
        ("in:anywhere", "in:anywhere"),
    ] {
        assert_eq!(
            strict_err(rule),
            format!("{term} isn't supported in split rules"),
            "{rule}"
        );
    }
    let err = Query::parse_rule("a -b").unwrap_err();
    assert_eq!(err.offset(), 2);
}

#[test]
fn unknown_prefixes_are_plain_words() {
    assert_eq!(p("re:budget 10:30 "), r#"(and "re:budget" "10:30")"#);
    assert_eq!(p("+exact "), r#""exact""#);
}

#[test]
fn unicode_words() {
    assert_eq!(
        p("from:Zoë Привет 日本 "),
        r#"(and from:"Zoë" "Привет" "日本")"#
    );
}

#[test]
fn lenient_parse_degrades_gracefully() {
    assert_eq!(p("(a OR b"), r#"(or "a" "b"*)"#);
    assert_eq!(p("a) b "), r#"(and "a" "b")"#);
    assert_eq!(p("a OR"), r#""a""#);
    assert_eq!(p("OR a "), r#""a""#);
    assert_eq!(p("a OR OR b "), r#"(or "a" "b")"#);
    assert_eq!(p("a AND"), r#""a""#);
    assert_eq!(p("from:"), "(and)");
    assert_eq!(p("from: bob "), r#""bob""#);
    assert_eq!(p("() a "), r#""a""#);
    assert_eq!(p("a ()"), r#""a""#);
    assert_eq!(p("& - ' "), "(and)");
    assert_eq!(p(")))((("), "(and)");
    assert_eq!(p(r#"""#), "(and)");
    assert_eq!(p("::: "), "(and)");
}

#[test]
fn garbage_never_panics() {
    let inputs = [
        r#"(("""::-(-("#,
        "from:\"",
        "-",
        "-(",
        "label:\"\"",
        "is:",
        "\u{0}\u{1}",
        "🙂 OR 🙂",
        "a\"b\"c:d(e",
        "from:\"a\"\"b\"",
        "ÄND OR ÖR",
    ];
    for input in inputs {
        let q = Query::parse(input);
        // Only that each returns: what it returns is not checked here.
        _ = q.to_fts5();
        _ = Query::parse_rule(input);
        _ = gmail_query(input);
    }
}

#[test]
fn strict_errors() {
    assert_eq!(strict_err("(a OR b"), "Missing )");
    assert_eq!(strict_err("a) b"), "Unmatched )");
    assert_eq!(strict_err("a OR"), "OR needs a term after it");
    assert_eq!(strict_err("OR a"), "OR needs a term before it");
    assert_eq!(strict_err("a AND"), "AND needs a term on both sides");
    assert_eq!(strict_err("AND a"), "AND needs a term on both sides");
    assert_eq!(strict_err("from:"), "from: needs a value");
    assert_eq!(strict_err("a ()"), "Empty parentheses");
    // The first problem by position wins, though the parser finds the dangling OR first.
    let err = Query::parse_rule("(a OR").unwrap_err();
    assert_eq!((err.to_string().as_str(), err.offset()), ("Missing )", 0));
}

#[test]
fn strict_accepts_valid_rules() {
    for rule in [
        "is:list",
        "label:CATEGORY_PROMOTIONS",
        r#"from:alice OR (subject:"q3 plan" AND to:team)"#,
        "from:@example.com",
    ] {
        assert!(Query::parse_rule(rule).is_ok(), "{rule}");
    }
}

// --- matching -------------------------------------------------------

fn msg() -> MessageFacts {
    MessageFacts {
        to: vec![Addr::named("Bob", "bob@corp.io")],
        cc: vec![Addr::new("carol@corp.io")],
        bcc: vec![Addr::named("Dan", "dan@secret.org")],
        subject: "Q3 Plan: Budget review".into(),
        snippet: "Quarterly numbers inside".into(),
        ..message(
            1,
            Addr::named("Alice Smith", "alice@example.com"),
            &["INBOX", "CATEGORY_UPDATES", "Label_7"],
        )
    }
}

fn thread_labels() -> Vec<&'static str> {
    vec!["Work/Projects", "Clients"]
}

const BODY: &str = "Please read the attached draft.";

#[derive(Clone, Copy)]
enum Mode {
    Rule,
    Search,
}

fn me() -> Identity {
    Identity::new("ana@acme.test".into(), "Ana".into(), Vec::new())
}

fn matches_msg(q: &str, m: &MessageFacts, mode: Mode) -> bool {
    let labels = thread_labels();
    let me = me();
    let target = match mode {
        Mode::Rule => MatchTarget::rule(m, &labels),
        Mode::Search => MatchTarget::search(m, &labels, BODY, false, &me),
    };
    let query = match mode {
        Mode::Rule => Query::parse(q),
        Mode::Search => search(q),
    };
    query.matches(&target)
}

fn m(q: &str, mode: Mode) -> bool {
    matches_msg(q, &msg(), mode)
}

#[test]
fn address_operators_match_name_or_email() {
    for q in [
        "from:alice",
        "from:SMITH",
        "from:example.com",
        "from:\"alice smith\"",
    ] {
        assert!(m(q, Mode::Rule), "{q}");
    }
    assert!(!m("from:bob", Mode::Rule));
    assert!(m("to:bob", Mode::Rule));
    assert!(m("to:corp.io", Mode::Rule));
    assert!(!m("to:carol", Mode::Rule));
    assert!(m("cc:carol", Mode::Rule));
    assert!(m("bcc:secret", Mode::Rule));
    assert!(!m("bcc:bob", Mode::Rule));
}

#[test]
fn bare_words_differ_by_mode() {
    assert!(m("alice", Mode::Rule));
    assert!(m("smith", Mode::Rule), "the sender's display name");
    assert!(m("budget", Mode::Rule));
    assert!(m("\"q3 plan\"", Mode::Rule));
    assert!(!m("attached", Mode::Rule));
    assert!(m("attached", Mode::Search));
    // A bare word does not look at recipients.
    assert!(!m("bob", Mode::Search));
    // Nor at the snippet: search reads the stored body.
    assert!(!m("quarterly", Mode::Search));
}

#[test]
fn subject_operator() {
    assert!(m("subject:budget", Mode::Rule));
    assert!(!m("subject:alice", Mode::Rule));
}

#[test]
fn label_operator() {
    // User labels: thread label names, ignoring case; Gmail's hyphen form works.
    assert!(m("label:clients", Mode::Rule));
    assert!(m("label:work-projects", Mode::Rule));
    assert!(m("label:\"work/projects\"", Mode::Rule));
    assert!(!m("label:work", Mode::Rule));
    // System and category ids: this message's own label ids.
    assert!(m("label:inbox", Mode::Rule));
    assert!(m("label:CATEGORY_UPDATES", Mode::Rule));
    assert!(m("label:category_updates", Mode::Rule));
    // Spaces around a quoted value don't matter, for system ids as for names.
    assert!(m("label:\" inbox \"", Mode::Rule));
    assert!(m("label:\"category_updates \"", Mode::Rule));
    assert!(m("label:\" clients \"", Mode::Rule));
    assert!(!m("label:CATEGORY_PROMOTIONS", Mode::Rule));
    assert!(!m("label:starred", Mode::Rule));
    // User label ids are not names.
    assert!(!m("label:Label_7", Mode::Rule));
}

#[test]
fn is_list_operator() {
    assert!(!m("is:list", Mode::Rule));
    let list = MessageFacts {
        is_list: true,
        ..msg()
    };
    assert!(matches_msg("is:list", &list, Mode::Rule));
}

#[test]
fn boolean_structure() {
    assert!(m("from:bob OR from:alice", Mode::Rule));
    assert!(!m("from:bob OR (from:alice subject:nope)", Mode::Rule));
    assert!(m("(from:bob OR from:alice) subject:budget", Mode::Rule));
    assert!(!m("from:bob from:alice", Mode::Rule));
}

#[test]
fn empty_matches_everything_and_unsupported_nothing() {
    assert!(m("", Mode::Rule));
    assert!(!m("filename:pdf", Mode::Search));
}

#[test]
fn label_operators_in_search() {
    // msg() is INBOX, CATEGORY_UPDATES, and Label_7, in a thread labeled
    // Work/Projects and Clients.
    assert!(m("in:inbox", Mode::Search));
    assert!(m("in:INBOX alice", Mode::Search));
    assert!(!m("in:sent", Mode::Search));
    assert!(!m("in:sent alice", Mode::Search), "the reported bug");
    assert!(m("in:work-projects", Mode::Search));
    assert!(m("category:updates", Mode::Search));
    assert!(!m("category:primary", Mode::Search));
    assert!(!m("is:unread", Mode::Search));
    let unread_starred = MessageFacts {
        labels: crate::test_support::set(&["INBOX", "UNREAD", "STARRED", "IMPORTANT"]),
        ..msg()
    };
    for q in ["is:unread", "is:starred", "is:important", "in:starred"] {
        assert!(matches_msg(q, &unread_starred, Mode::Search), "{q}");
        assert!(!m(q, Mode::Search), "{q}");
    }
    assert!(!matches_msg("is:read", &unread_starred, Mode::Search));
    assert!(m("is:read", Mode::Search));
}

#[test]
fn negation_matches_the_complement() {
    assert!(!m("-alice", Mode::Search));
    assert!(m("-bob", Mode::Search));
    assert!(m("-in:sent", Mode::Search));
    assert!(!m("-in:inbox", Mode::Search));
    assert!(m("budget -(bob OR carol)", Mode::Search));
    assert!(!m("budget -(bob OR alice)", Mode::Search));
    // The body counts for a negated word, as for a bare one.
    assert!(!m("-attached", Mode::Search));
}

#[test]
fn has_attachment_and_snoozed() {
    assert!(!m("has:attachment", Mode::Search));
    let with = MessageFacts {
        has_attachments: true,
        ..msg()
    };
    assert!(matches_msg("has:attachment", &with, Mode::Search));
    let labels = thread_labels();
    let me = me();
    let message = msg();
    let snoozed = MatchTarget::search(&message, &labels, BODY, true, &me);
    let awake = MatchTarget::search(&message, &labels, BODY, false, &me);
    for q in ["in:snoozed", "is:snoozed"] {
        assert!(search(q).matches(&snoozed), "{q}");
        assert!(!search(q).matches(&awake), "{q}");
    }
    assert!(search("in:anywhere").matches(&awake));
}

#[test]
fn me_is_this_accounts_addresses_in_search_only() {
    let to_me = MessageFacts {
        to: vec![Addr::new("ANA@acme.test")],
        ..msg()
    };
    assert!(matches_msg("to:me", &to_me, Mode::Search));
    assert!(!matches_msg("from:me", &to_me, Mode::Search));
    assert!(!matches_msg("cc:me", &to_me, Mode::Search));
    let from_me = MessageFacts {
        from: Addr::new("ana@acme.test"),
        bcc: vec![Addr::new("ana@acme.test")],
        ..msg()
    };
    assert!(matches_msg("from:me", &from_me, Mode::Search));
    assert!(matches_msg("bcc:me", &from_me, Mode::Search));
    // A rule keeps reading `me` as text, as it always has.
    let named = MessageFacts {
        from: Addr::new("me@example.com"),
        ..msg()
    };
    assert!(matches_msg("from:me", &named, Mode::Rule));
    assert!(!matches_msg("from:me", &named, Mode::Search));
    // The index has no `me` to look up.
    assert_eq!(
        search("from:me x ").to_fts5().as_deref(),
        Some(r#"{from_text subject body} : "x""#)
    );
}

#[test]
fn dates_are_local_days_as_in_gmail() {
    let midnight = ny(2026, 11, 2, 0);
    assert_eq!(s("after:2026/11/02 "), format!("after:{midnight}"));
    assert_eq!(s("after:2026-11-2 "), format!("after:{midnight}"));
    assert_eq!(s("newer:11/02/2026 "), format!("after:{midnight}"));
    assert_eq!(s("before:2026/11/02 "), format!("before:{midnight}"));
    assert_eq!(s("older:2026/11/02 "), format!("before:{midnight}"));
    // Before the DST change: midnight is at -04:00, not today's -05:00.
    assert_eq!(
        s("after:2026/10/31 "),
        format!("after:{}", ny(2026, 10, 31, 0))
    );
    assert_eq!(ny(2026, 10, 31, 0), 1_793_419_200_000);
    // A bare number is seconds since the epoch.
    assert_eq!(s("after:1793419200 "), "after:1793419200000");
    // Ages count back calendar days, months, and years from now.
    assert_eq!(
        s("newer_than:2d "),
        format!("after:{}", ny(2026, 10, 31, 10))
    );
    assert_eq!(
        s("older_than:1M "),
        format!("before:{}", ny(2026, 10, 2, 10))
    );
    assert_eq!(
        s("older_than:1y "),
        format!("before:{}", ny(2025, 11, 2, 10))
    );
}

#[test]
fn date_bounds_include_after_and_exclude_before() {
    let at = |ms: i64| MessageFacts {
        date: crate::Millis(ms),
        ..msg()
    };
    let midnight = ny(2026, 11, 2, 0);
    for (q, ms, want) in [
        ("after:2026/11/02", midnight, true),
        ("after:2026/11/02", midnight - 1, false),
        ("before:2026/11/02", midnight - 1, true),
        ("before:2026/11/02", midnight, false),
        ("newer_than:2d", ny(2026, 10, 31, 10), true),
        ("newer_than:2d", ny(2026, 10, 31, 9), false),
        ("older_than:2d", ny(2026, 10, 31, 9), true),
        ("after:2026/11/01 before:2026/11/02", midnight - 1, true),
        ("-after:2026/11/02", midnight, false),
    ] {
        assert_eq!(matches_msg(q, &at(ms), Mode::Search), want, "{q} at {ms}");
    }
}

#[test]
fn scope_widens_only_when_named() {
    use crate::query::Scope;
    let scope = |q: &str| search(q).scope();
    assert!(scope("foo in:inbox -in:sent").live_only());
    assert_eq!(
        scope("in:trash foo"),
        Scope {
            trash: true,
            ..Scope::default()
        }
    );
    assert_eq!(
        scope("label:SPAM"),
        Scope {
            spam: true,
            ..Scope::default()
        }
    );
    assert_eq!(
        scope("-in:trash"),
        Scope {
            trash: true,
            ..Scope::default()
        }
    );
    assert_eq!(
        scope("in:drafts"),
        Scope {
            drafts: true,
            ..Scope::default()
        }
    );
    assert_eq!(
        scope("in:anywhere x"),
        Scope {
            trash: true,
            spam: true,
            drafts: false
        }
    );
    let trashed = MessageFacts {
        labels: crate::test_support::set(&["TRASH"]),
        ..msg()
    };
    let spam = MessageFacts {
        labels: crate::test_support::set(&["SPAM"]),
        ..msg()
    };
    let draft = MessageFacts {
        is_draft: true,
        ..msg()
    };
    let live = Scope::default();
    assert!(live.admits(&msg()));
    assert!(!live.admits(&trashed) && !live.admits(&spam) && !live.admits(&draft));
    assert!(scope("in:trash").admits(&trashed) && !scope("in:trash").admits(&spam));
    assert!(scope("in:spam").admits(&spam) && !scope("in:spam").admits(&trashed));
    assert!(scope("in:drafts").admits(&draft));
    assert!(scope("in:anywhere").admits(&trashed) && scope("in:anywhere").admits(&spam));
}

#[test]
fn unicode_matching_is_case_insensitive() {
    let fields = MessageFacts {
        from: Addr::named("Ölaf Ärger", "olaf@exämple.de"),
        subject: "ПРИВЕТ мир".into(),
        ..msg()
    };
    assert!(matches_msg("from:ölaf", &fields, Mode::Rule));
    assert!(matches_msg("from:EXÄMPLE", &fields, Mode::Rule));
    assert!(matches_msg("привет", &fields, Mode::Rule));
}

// --- FTS5 -----------------------------------------------------------

fn fts(q: &str) -> Option<String> {
    Query::parse(q).to_fts5()
}

#[test]
fn fts_columns() {
    assert_eq!(
        fts("from:alice to:bob cc:carol subject:hi word ").as_deref(),
        Some(
            r#"from_text : "alice" AND to_text : "bob" AND cc_text : "carol" AND subject : "hi" AND {from_text subject body} : "word""#
        )
    );
}

#[test]
fn fts_prefix_on_last_word_only() {
    assert_eq!(
        fts("hello wor").as_deref(),
        Some(r#"{from_text subject body} : "hello" AND {from_text subject body} : "wor"*"#)
    );
    assert_eq!(fts("from:ali").as_deref(), Some(r#"from_text : "ali"*"#));
    assert_eq!(
        fts(r#""q3 plan""#).as_deref(),
        Some(r#"{from_text subject body} : "q3 plan""#)
    );
    assert_eq!(
        fts(r#""q3 pl"#).as_deref(),
        Some(r#"{from_text subject body} : "q3 pl"*"#)
    );
}

#[test]
fn fts_drops_what_it_cannot_express() {
    assert_eq!(fts("label:work"), None);
    assert_eq!(fts("is:list"), None);
    assert_eq!(fts("bcc:dan"), None);
    assert_eq!(fts("has:attachment"), None);
    assert_eq!(fts(""), None);
    assert_eq!(
        fts("is:list from:a ").as_deref(),
        Some(r#"from_text : "a""#)
    );
    // OR with an inexpressible side can't narrow anything.
    assert_eq!(fts("from:a OR label:x "), None);
    assert_eq!(
        fts("(from:a OR label:x) b ").as_deref(),
        Some(r#"{from_text subject body} : "b""#)
    );
    assert!(Query::parse("label:x a").needs_post_filter());
    assert!(Query::parse("a OR bcc:b").needs_post_filter());
    assert!(Query::parse("is:list").needs_post_filter());
    assert!(Query::parse("a has:attachment").needs_post_filter());
    assert!(Query::parse("a -b").needs_post_filter());
    assert!(Query::parse("a in:sent").needs_post_filter());
    assert!(Query::parse("a in:anywhere").needs_post_filter());
    assert!(!Query::parse("from:a (b OR subject:c)").needs_post_filter());
    assert_eq!(
        fts("a -b ").as_deref(),
        Some(r#"{from_text subject body} : "a""#)
    );
    assert_eq!(fts("a OR -b "), None);
}

#[test]
fn fts_nesting_is_parenthesised() {
    assert_eq!(
        fts("(a OR b) c ").as_deref(),
        Some(
            r#"({from_text subject body} : "a" OR {from_text subject body} : "b") AND {from_text subject body} : "c""#
        )
    );
    assert_eq!(
        fts("a b OR c ").as_deref(),
        Some(
            r#"({from_text subject body} : "a" AND {from_text subject body} : "b") OR {from_text subject body} : "c""#
        )
    );
}

#[test]
fn fts_escapes_special_characters() {
    assert_eq!(
        fts(r#"a"b "#).as_deref(),
        Some(r#"{from_text subject body} : "a""b""#)
    );
    assert_eq!(
        fts("foo* ").as_deref(),
        Some(r#"{from_text subject body} : "foo*""#)
    );
    assert_eq!(
        fts("re:x ").as_deref(),
        Some(r#"{from_text subject body} : "re:x""#)
    );
    assert_eq!(
        fts("e-mail ").as_deref(),
        Some(r#"{from_text subject body} : "e-mail""#)
    );
    assert_eq!(
        fts("NEAR ").as_deref(),
        Some(r#"{from_text subject body} : "NEAR""#)
    );
    assert_eq!(
        fts("a\u{0}b ").as_deref(),
        Some(r#"{from_text subject body} : "a b""#)
    );
    // Nothing tokenizable: no term rather than an empty phrase.
    assert_eq!(fts("*** ^ "), None);
}

#[test]
fn searches_locally_unless_empty_or_unsupported() {
    assert!(!Query::parse("").searches_locally());
    assert!(!Query::parse("filename:pdf x").searches_locally());
    assert!(Query::parse("filename:pdf x").needs_gmail());
    assert!(!Query::parse("x OR -larger:1M").searches_locally());
    assert!(Query::parse("has:attachment -x").searches_locally());
    assert!(!Query::parse("has:attachment -x").needs_gmail());
    assert!(Query::parse("is:list").searches_locally());
    assert!(search("after:2026/01/31").searches_locally());
}

#[test]
fn reads_body_only_for_bare_words() {
    assert!(Query::parse("hello").reads_body());
    assert!(Query::parse("from:a OR \"q3 plan\"").reads_body());
    assert!(!Query::parse("from:a label:x is:list").reads_body());
    assert!(!Query::parse("has:attachment").reads_body());
}

/// Every FTS5 string we produce must be accepted by SQLite and find the row.
#[test]
fn fts_output_is_valid_sqlite() {
    let db = rusqlite::Connection::open_in_memory().unwrap();
    db.execute_batch(&format!(
        "CREATE VIRTUAL TABLE t USING fts5({});
         INSERT INTO t VALUES ('Alice Smith alice@example.com', 'Bob bob@corp.io',
           'carol@corp.io', 'Q3 plan: budget', 'The e-mail said \"hello\" *loudly* NEAR AND OR');",
        FTS_COLUMNS.join(", ")
    ))
    .unwrap();
    let count = |q: &str| -> i64 {
        db.query_row("SELECT count(*) FROM t WHERE t MATCH ?1", [q], |r| r.get(0))
            .unwrap_or_else(|e| panic!("{q}: {e}"))
    };
    let cases = [
        ("from:alice", 1),
        ("from:ali", 1),
        ("from:alice@example.com ", 1),
        ("to:bob", 1),
        ("cc:carol", 1),
        ("subject:\"q3 plan\"", 1),
        ("subject:\"q3 pl", 1),
        ("hello", 1),
        ("\"said hello\"", 1),
        ("e-mail ", 1),
        ("budget OR nothing ", 1),
        ("(alice OR zed) budg", 1),
        ("from:bob", 0),
        ("NEAR AND OR ", 1),
        ("loudly* ", 1),
        ("a\"b ", 0),
        ("\"un\"\"closed", 0),
        ("x:y:z ", 0),
        ("{from_text} ", 0),
        ("^start ", 0),
        ("col:umn ", 0),
        ("日本語 ", 0),
        ("from:\"a\"\"b\"", 0),
        ("( ( (", 0),
    ];
    for (input, want) in cases {
        if let Some(expr) = fts(input) {
            assert_eq!(count(&expr), want, "{input} -> {expr}");
        } else {
            assert_eq!(want, 0, "{input} produced no FTS expression");
        }
    }
}

// --- the Gmail query ------------------------------------------------

#[test]
fn gmail_query_strips_is_list() {
    assert_eq!(gmail_query("from:a b"), "from:a b");
    assert_eq!(gmail_query("is:list"), "");
    assert_eq!(gmail_query("IS:LIST from:a"), "from:a");
    assert_eq!(gmail_query("from:a is:list b"), "from:a b");
    assert_eq!(gmail_query("is:list OR from:a"), "from:a");
    assert_eq!(gmail_query("from:a OR is:list"), "from:a");
    assert_eq!(gmail_query("a OR is:list OR b"), "a OR b");
    assert_eq!(gmail_query("(is:list) a"), "a");
    assert_eq!(gmail_query("(is:list OR x) a"), "(x) a");
    assert_eq!(gmail_query("-is:list a"), "a");
    assert_eq!(gmail_query("-is:list -b"), "-b");
    assert_eq!(gmail_query("-b is:list"), "-b");
    assert_eq!(gmail_query("-(is:list) a"), "a");
    assert_eq!(gmail_query("\"is:list\" a"), "\"is:list\" a");
    assert_eq!(gmail_query("is:listing a"), "is:listing a");
    assert_eq!(gmail_query("has:attachment is:list"), "has:attachment");
}
