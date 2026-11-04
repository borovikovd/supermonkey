//! Cases ported from the old `ub-rules` query tests (spec-tests/ub-rules/query.rs).

use super::*;
use crate::test_support::message;
use crate::{Addr, MessageFacts};

fn p(s: &str) -> String {
    Query::parse(s).to_string()
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
        p("has:attachment after:2024/01/01 -foo in:inbox is:unread x "),
        r#"(and ?has:attachment ?after:2024/01/01 ?-foo ?in:inbox ?is:unread "x")"#
    );
    assert_eq!(p("-(a OR b) c "), r#"(and ?-(a OR b) "c")"#);
    assert_eq!(p("from:(a b) c "), r#"(and ?from:(a b) "c")"#);
    assert_eq!(p(r#"-"a b" c "#), r#"(and ?-"a b" "c")"#);
    assert_eq!(
        strict_err("has:attachment"),
        "has:attachment isn't supported in split rules"
    );
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
        let _ = q.to_fts5();
        let _ = Query::parse_rule(input);
        let _ = gmail_query(input);
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

fn thread_labels() -> Vec<String> {
    vec!["Work/Projects".into(), "Clients".into()]
}

const BODY: &str = "Please read the attached draft.";

#[derive(Clone, Copy)]
enum Mode {
    Rule,
    Search,
}

fn matches_msg(q: &str, m: &MessageFacts, mode: Mode) -> bool {
    let labels = thread_labels();
    let target = match mode {
        Mode::Rule => MatchTarget::rule(m, &labels),
        Mode::Search => MatchTarget::search(m, &labels, BODY),
    };
    Query::parse(q).matches(&target)
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
fn unsupported_and_empty_match_everything() {
    assert!(m("", Mode::Rule));
    assert!(m("has:attachment", Mode::Search));
    assert!(m("-alice", Mode::Search));
    assert!(!m("has:attachment nope", Mode::Search));
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
    assert!(!Query::parse("a has:attachment").needs_post_filter());
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
fn has_local_terms() {
    assert!(!Query::parse("").has_local_terms());
    assert!(!Query::parse("has:attachment -x").has_local_terms());
    assert!(Query::parse("has:attachment x").has_local_terms());
    assert!(Query::parse("is:list").has_local_terms());
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
    assert_eq!(gmail_query("\"is:list\" a"), "\"is:list\" a");
    assert_eq!(gmail_query("is:listing a"), "is:listing a");
    assert_eq!(gmail_query("has:attachment is:list"), "has:attachment");
}
