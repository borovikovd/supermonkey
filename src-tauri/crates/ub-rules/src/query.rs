//! The custom-rule and local-search query language.
//!
//! Supported: `from:`, `to:`, `cc:`, `bcc:`, `subject:`, `label:`, `is:list`,
//! bare words, `"quoted phrases"`, `AND`, `OR`, and parentheses. Terms next to
//! each other are AND, and AND binds tighter than OR. Operator names, `AND`,
//! `OR`, and all matching are case-insensitive.
//!
//! Other Gmail syntax (`has:attachment`, `after:2024/01/01`, `-foo`, `in:inbox`,
//! `from:(a b)`, ...) parses to [`Expr::Unsupported`], which always matches, so
//! local search ignores it while the raw string still goes to Gmail.
//!
//! [`parse`] never fails: unbalanced parentheses, an unclosed quote, a dangling
//! `OR`, or an operator with no value yet degrade to a best-effort query.
//! [`parse_strict`] reports those as a [`ParseError`] for the split settings
//! sheet, and also rejects unsupported terms, since a rule never goes to Gmail.

use std::fmt;

/// Column filter for bare words in the FTS5 table.
const FTS_ANY_COLUMNS: &str = "{from_text subject body}";

/// Gmail operators that this language does not evaluate. A word starting with
/// one of these followed by `:` becomes [`Expr::Unsupported`]. Any other
/// `word:word` (like `re:budget` or `10:30`) is a plain bare word.
const GMAIL_ONLY_OPERATORS: &[&str] = &[
    "has",
    "in",
    "after",
    "before",
    "older",
    "newer",
    "older_than",
    "newer_than",
    "filename",
    "larger",
    "smaller",
    "size",
    "category",
    "list",
    "deliveredto",
    "rfc822msgid",
    "around",
];

/// Gmail system label ids that `label:` matches against a message's own label
/// ids. Anything starting with `CATEGORY_` counts too.
const SYSTEM_LABEL_IDS: &[&str] = &[
    "INBOX",
    "SPAM",
    "TRASH",
    "UNREAD",
    "STARRED",
    "IMPORTANT",
    "SENT",
    "DRAFT",
    "CHAT",
];

/// Which part of a message a [`Term`] looks at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    /// A bare word or phrase: sender or subject, plus body in [`Mode::Search`].
    Any,
    From,
    To,
    Cc,
    Bcc,
    Subject,
    Label,
}

/// One `field:value` test, or a bare word or phrase when `field` is [`Field::Any`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Term {
    pub field: Field,
    /// The value as typed, without surrounding quotes.
    pub value: String,
    /// True for the last term when the user may still be typing it (no
    /// trailing space and no closing quote). [`Query::to_fts5`] turns it into
    /// a prefix match.
    pub prefix: bool,
}

/// A parsed query expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    /// Every child must match. `And(vec![])` is the empty query and matches everything.
    And(Vec<Expr>),
    /// At least one child must match.
    Or(Vec<Expr>),
    Term(Term),
    /// `is:list`: the message has a `List-Id` or `List-Unsubscribe` header.
    IsList,
    /// Syntax this language does not evaluate, kept verbatim. Always matches.
    Unsupported(String),
}

/// How bare words match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Split rules: a bare word matches the sender's name or address, or the subject.
    Rule,
    /// Local search: a bare word also matches the body text.
    Search,
}

/// Why [`parse_strict`] rejected a query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// A short sentence to show under the rule field.
    pub message: String,
    /// Byte offset in the input where the problem is.
    pub offset: usize,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ParseError {}

/// The message a rule or search is tested against.
///
/// Addresses are `(display name, email address)` pairs; the name may be empty.
pub trait RuleSubject {
    /// The From address.
    fn sender(&self) -> (&str, &str);
    fn to(&self) -> Vec<(&str, &str)>;
    fn cc(&self) -> Vec<(&str, &str)>;
    fn bcc(&self) -> Vec<(&str, &str)>;
    fn subject(&self) -> &str;
    /// Plain body text. Only read in [`Mode::Search`].
    fn body(&self) -> &str;
    /// Names of the thread's user labels (not ids), from any of its messages.
    fn thread_user_labels(&self) -> Vec<&str>;
    /// This message's own Gmail label ids, such as `INBOX` or `CATEGORY_SOCIAL`.
    fn message_label_ids(&self) -> Vec<&str>;
    /// The message has a `List-Id` or `List-Unsubscribe` header.
    fn is_list(&self) -> bool;
}

/// A display name and email address.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Address {
    pub name: String,
    pub email: String,
}

impl Address {
    pub fn new(name: impl Into<String>, email: impl Into<String>) -> Self {
        Address {
            name: name.into(),
            email: email.into(),
        }
    }
}

/// An owned [`RuleSubject`], handy when the fields are already loaded.
#[derive(Debug, Clone, Default)]
pub struct MessageFields {
    pub from: Address,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub bcc: Vec<Address>,
    pub subject: String,
    pub body: String,
    pub thread_user_labels: Vec<String>,
    pub message_label_ids: Vec<String>,
    pub is_list: bool,
}

fn pairs(addresses: &[Address]) -> Vec<(&str, &str)> {
    addresses
        .iter()
        .map(|a| (a.name.as_str(), a.email.as_str()))
        .collect()
}

impl RuleSubject for MessageFields {
    fn sender(&self) -> (&str, &str) {
        (&self.from.name, &self.from.email)
    }
    fn to(&self) -> Vec<(&str, &str)> {
        pairs(&self.to)
    }
    fn cc(&self) -> Vec<(&str, &str)> {
        pairs(&self.cc)
    }
    fn bcc(&self) -> Vec<(&str, &str)> {
        pairs(&self.bcc)
    }
    fn subject(&self) -> &str {
        &self.subject
    }
    fn body(&self) -> &str {
        &self.body
    }
    fn thread_user_labels(&self) -> Vec<&str> {
        self.thread_user_labels.iter().map(String::as_str).collect()
    }
    fn message_label_ids(&self) -> Vec<&str> {
        self.message_label_ids.iter().map(String::as_str).collect()
    }
    fn is_list(&self) -> bool {
        self.is_list
    }
}

/// A parsed query. Build one with [`parse`] or [`parse_strict`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    pub expr: Expr,
}

/// Parses leniently, for search-as-you-type. Never fails.
pub fn parse(input: &str) -> Query {
    Parser::new(input).run().0
}

/// Parses a split rule, rejecting anything [`parse`] would have to guess at:
/// an empty query, unbalanced parentheses, an unclosed quote, a dangling
/// `AND`/`OR`, an operator with no value, or an unsupported term.
pub fn parse_strict(input: &str) -> Result<Query, ParseError> {
    let (query, errors) = Parser::new(input).run();
    if let Some(first) = errors.into_iter().min_by_key(|e| e.offset) {
        return Err(first);
    }
    if query.expr == Expr::And(Vec::new()) {
        return Err(ParseError {
            message: "Enter a query".into(),
            offset: 0,
        });
    }
    Ok(query)
}

impl Query {
    /// Lenient parse; same as [`parse`].
    pub fn parse(input: &str) -> Query {
        parse(input)
    }

    /// Strict parse; same as [`parse_strict`].
    pub fn parse_strict(input: &str) -> Result<Query, ParseError> {
        parse_strict(input)
    }

    /// Whether the message matches. Substring matching, case-insensitive.
    pub fn matches<S: RuleSubject + ?Sized>(&self, subject: &S, mode: Mode) -> bool {
        self.expr.matches(subject, mode)
    }

    /// An SQLite FTS5 `MATCH` expression over columns `from_text`, `to_text`,
    /// `cc_text`, `subject`, `body`, or `None` when nothing in the query can
    /// narrow the FTS index (for example only `label:` or `is:list`).
    ///
    /// The result selects a superset of the matching messages. Terms FTS cannot
    /// express (`bcc:`, `label:`, `is:list`, unsupported operators) are dropped
    /// from an AND; an OR containing one gives up and is dropped as a whole.
    /// When [`Query::needs_post_filter`] is true, filter the candidates with
    /// [`Query::matches`] in [`Mode::Search`].
    pub fn to_fts5(&self) -> Option<String> {
        fts(&self.expr).map(|(s, _)| s)
    }

    /// True when the query has `bcc:`, `label:` or `is:list` terms, which
    /// [`Query::to_fts5`] cannot express.
    pub fn needs_post_filter(&self) -> bool {
        fn walk(e: &Expr) -> bool {
            match e {
                Expr::And(xs) | Expr::Or(xs) => xs.iter().any(walk),
                Expr::Term(t) => matches!(t.field, Field::Bcc | Field::Label),
                Expr::IsList => true,
                Expr::Unsupported(_) => false,
            }
        }
        walk(&self.expr)
    }

    /// False when the query is empty or holds only unsupported terms, so local
    /// search has nothing to go on and should leave results to Gmail.
    pub fn has_local_terms(&self) -> bool {
        fn walk(e: &Expr) -> bool {
            match e {
                Expr::And(xs) | Expr::Or(xs) => xs.iter().any(walk),
                Expr::Term(_) | Expr::IsList => true,
                Expr::Unsupported(_) => false,
            }
        }
        walk(&self.expr)
    }
}

impl fmt::Display for Query {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.expr.fmt(f)
    }
}

/// A compact s-expression, mainly for tests and logs:
/// `(or (and from:"alice" subject:"q3 plan") "bob"*)`.
impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let list = |f: &mut fmt::Formatter<'_>, op: &str, xs: &[Expr]| {
            write!(f, "({op}")?;
            for x in xs {
                write!(f, " {x}")?;
            }
            f.write_str(")")
        };
        match self {
            Expr::And(xs) => list(f, "and", xs),
            Expr::Or(xs) => list(f, "or", xs),
            Expr::Term(t) => {
                let name = match t.field {
                    Field::Any => "",
                    Field::From => "from:",
                    Field::To => "to:",
                    Field::Cc => "cc:",
                    Field::Bcc => "bcc:",
                    Field::Subject => "subject:",
                    Field::Label => "label:",
                };
                let star = if t.prefix { "*" } else { "" };
                write!(f, "{name}\"{}\"{star}", t.value)
            }
            Expr::IsList => f.write_str("is:list"),
            Expr::Unsupported(raw) => write!(f, "?{raw}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Matching

fn contains_ci(haystack: &str, needle_lower: &str) -> bool {
    haystack.to_lowercase().contains(needle_lower)
}

fn address_matches(addr: (&str, &str), needle_lower: &str) -> bool {
    contains_ci(addr.0, needle_lower) || contains_ci(addr.1, needle_lower)
}

/// Gmail writes `Work/Projects` and `Work Projects` as `label:work-projects`.
fn normalize_label(name: &str) -> String {
    name.trim()
        .chars()
        .flat_map(char::to_lowercase)
        .map(|c| if c == ' ' || c == '/' { '-' } else { c })
        .collect()
}

fn is_system_label_id(value: &str) -> bool {
    let upper = value.to_ascii_uppercase();
    upper.starts_with("CATEGORY_") || SYSTEM_LABEL_IDS.contains(&upper.as_str())
}

impl Expr {
    fn matches<S: RuleSubject + ?Sized>(&self, s: &S, mode: Mode) -> bool {
        match self {
            Expr::And(xs) => xs.iter().all(|x| x.matches(s, mode)),
            Expr::Or(xs) => xs.iter().any(|x| x.matches(s, mode)),
            Expr::Term(t) => t.matches(s, mode),
            Expr::IsList => s.is_list(),
            Expr::Unsupported(_) => true,
        }
    }
}

impl Term {
    fn matches<S: RuleSubject + ?Sized>(&self, s: &S, mode: Mode) -> bool {
        let needle = self.value.to_lowercase();
        let any = |list: Vec<(&str, &str)>| list.into_iter().any(|a| address_matches(a, &needle));
        match self.field {
            Field::Any => {
                address_matches(s.sender(), &needle)
                    || contains_ci(s.subject(), &needle)
                    || (mode == Mode::Search && contains_ci(s.body(), &needle))
            }
            Field::From => address_matches(s.sender(), &needle),
            Field::To => any(s.to()),
            Field::Cc => any(s.cc()),
            Field::Bcc => any(s.bcc()),
            Field::Subject => contains_ci(s.subject(), &needle),
            Field::Label => {
                if is_system_label_id(&self.value) {
                    s.message_label_ids()
                        .iter()
                        .any(|id| id.eq_ignore_ascii_case(self.value.trim()))
                } else {
                    let want = normalize_label(&self.value);
                    s.thread_user_labels()
                        .iter()
                        .any(|name| normalize_label(name) == want)
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// FTS5

/// An FTS5 string literal, or `None` when the value has no token characters
/// (an empty phrase is not a useful FTS5 term). Control characters become
/// spaces and `"` is doubled, so any input yields a valid literal.
fn fts_string(value: &str) -> Option<String> {
    if !value.chars().any(char::is_alphanumeric) {
        return None;
    }
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\"\""),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
    Some(out)
}

/// Returns the expression and whether it is compound (needs parentheses
/// when nested).
fn fts(e: &Expr) -> Option<(String, bool)> {
    match e {
        Expr::Term(t) => {
            let column = match t.field {
                Field::Any => FTS_ANY_COLUMNS,
                Field::From => "from_text",
                Field::To => "to_text",
                Field::Cc => "cc_text",
                Field::Subject => "subject",
                Field::Bcc | Field::Label => return None,
            };
            let literal = fts_string(&t.value)?;
            let star = if t.prefix { "*" } else { "" };
            Some((format!("{column} : {literal}{star}"), false))
        }
        Expr::IsList | Expr::Unsupported(_) => None,
        Expr::And(xs) => {
            let parts: Vec<_> = xs.iter().filter_map(fts).collect();
            join(parts, " AND ")
        }
        Expr::Or(xs) => {
            let parts: Option<Vec<_>> = xs.iter().map(fts).collect();
            join(parts?, " OR ")
        }
    }
}

fn join(parts: Vec<(String, bool)>, op: &str) -> Option<(String, bool)> {
    if parts.len() <= 1 {
        return parts.into_iter().next();
    }
    let joined = parts
        .into_iter()
        .map(|(s, compound)| if compound { format!("({s})") } else { s })
        .collect::<Vec<_>>()
        .join(op);
    Some((joined, true))
}

// ---------------------------------------------------------------------------
// Lexer

#[derive(Debug, Clone, PartialEq, Eq)]
enum TokKind {
    LParen,
    RParen,
    /// A bare `"quoted phrase"`.
    Phrase {
        text: String,
        closed: bool,
    },
    /// Anything else, including `op:value`, `op:"quoted value"` and `-negation`.
    Word(String),
}

#[derive(Debug, Clone)]
struct Tok {
    kind: TokKind,
    start: usize,
    end: usize,
}

impl Tok {
    fn is_keyword(&self, keyword: &str) -> bool {
        matches!(&self.kind, TokKind::Word(w) if w.eq_ignore_ascii_case(keyword))
    }
}

/// Index just past the closing quote of the quote at `start`, and whether it was closed.
fn scan_quote(s: &str, start: usize) -> (usize, bool) {
    match s[start + 1..].find('"') {
        Some(j) => (start + 1 + j + 1, true),
        None => (s.len(), false),
    }
}

/// Index just past the parenthesis matching the one at `start`, or the end.
fn scan_group(s: &str, start: usize) -> usize {
    let mut depth = 0usize;
    for (i, c) in s[start..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return start + i + 1;
                }
            }
            _ => {}
        }
    }
    s.len()
}

fn scan_word(s: &str, start: usize) -> usize {
    let negated = s[start..].starts_with('-');
    if s[start..].starts_with("-(") {
        return scan_group(s, start + 1);
    }
    let mut i = start;
    while let Some(c) = s[i..].chars().next() {
        // A quote or group right after `op:` or a leading `-` belongs to this word.
        let attached = s[..i].ends_with(':') || (negated && i == start + 1);
        match c {
            '"' if attached => i = scan_quote(s, i).0,
            '(' if attached => i = scan_group(s, i),
            c if c.is_whitespace() || c == '(' || c == ')' => break,
            c => i += c.len_utf8(),
        }
    }
    i
}

fn lex(s: &str) -> Vec<Tok> {
    let mut toks = Vec::new();
    let mut i = 0;
    while let Some(c) = s[i..].chars().next() {
        let start = i;
        let kind = match c {
            c if c.is_whitespace() => {
                i += c.len_utf8();
                continue;
            }
            '(' => {
                i += 1;
                TokKind::LParen
            }
            ')' => {
                i += 1;
                TokKind::RParen
            }
            '"' => {
                let (end, closed) = scan_quote(s, start);
                i = end;
                let inner_end = if closed { end - 1 } else { end };
                TokKind::Phrase {
                    text: s[start + 1..inner_end].to_owned(),
                    closed,
                }
            }
            _ => {
                i = scan_word(s, start);
                TokKind::Word(s[start..i].to_owned())
            }
        };
        toks.push(Tok {
            kind,
            start,
            end: i,
        });
    }
    toks
}

// ---------------------------------------------------------------------------
// Parser

struct Parser<'a> {
    input: &'a str,
    toks: Vec<Tok>,
    pos: usize,
    errors: Vec<ParseError>,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Parser {
            input,
            toks: lex(input),
            pos: 0,
            errors: Vec::new(),
        }
    }

    fn run(mut self) -> (Query, Vec<ParseError>) {
        let expr = self.parse_or(0).unwrap_or(Expr::And(Vec::new()));
        (Query { expr }, self.errors)
    }

    fn error(&mut self, offset: usize, message: impl Into<String>) {
        self.errors.push(ParseError {
            message: message.into(),
            offset,
        });
    }

    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn peek_is(&self, keyword: &str) -> bool {
        self.peek().is_some_and(|t| t.is_keyword(keyword))
    }

    /// `and_expr (OR and_expr)*`
    fn parse_or(&mut self, depth: usize) -> Option<Expr> {
        let mut alternatives = Vec::new();
        let first = self.parse_and(depth);
        if first.is_none() && self.peek_is("OR") {
            let offset = self.peek().map_or(0, |t| t.start);
            self.error(offset, "OR needs a term before it");
        }
        alternatives.extend(first);
        while self.peek_is("OR") {
            let offset = self.toks[self.pos].start;
            self.pos += 1;
            match self.parse_and(depth) {
                Some(e) => alternatives.push(e),
                None => self.error(offset, "OR needs a term after it"),
            }
        }
        combine(alternatives, false)
    }

    /// `unary ((AND)? unary)*`, stopping at OR, a closing parenthesis inside a
    /// group, or the end.
    fn parse_and(&mut self, depth: usize) -> Option<Expr> {
        let mut items = Vec::new();
        while let Some(tok) = self.peek().cloned() {
            if tok.is_keyword("OR") {
                break;
            }
            if tok.is_keyword("AND") {
                self.pos += 1;
                let right_missing = match self.peek() {
                    None => true,
                    Some(next) => next.is_keyword("OR") || next.kind == TokKind::RParen,
                };
                if items.is_empty() || right_missing {
                    self.error(tok.start, "AND needs a term on both sides");
                }
                continue;
            }
            match &tok.kind {
                TokKind::RParen if depth > 0 => break,
                TokKind::RParen => {
                    self.pos += 1;
                    self.error(tok.start, "Unmatched )");
                }
                TokKind::LParen => {
                    self.pos += 1;
                    let inner = self.parse_or(depth + 1);
                    if self.peek().is_some_and(|t| t.kind == TokKind::RParen) {
                        self.pos += 1;
                    } else {
                        self.error(tok.start, "Missing )");
                    }
                    match inner {
                        Some(e) => items.push(e),
                        None => self.error(tok.start, "Empty parentheses"),
                    }
                }
                TokKind::Phrase { text, closed } => {
                    self.pos += 1;
                    if !closed {
                        self.error(tok.start, "Missing closing quote");
                    }
                    if text.chars().any(char::is_alphanumeric) {
                        items.push(Expr::Term(Term {
                            field: Field::Any,
                            value: text.clone(),
                            prefix: !closed,
                        }));
                    }
                }
                TokKind::Word(word) => {
                    self.pos += 1;
                    let at_end = tok.end == self.input.len();
                    items.extend(self.word(word, tok.start, at_end));
                }
            }
        }
        combine(items, true)
    }

    /// What one word token means: `None` when there is nothing to match on, such as
    /// `from:` with no value or a lone `&`.
    fn word(&mut self, word: &str, offset: usize, at_end: bool) -> Option<Expr> {
        if word.len() > 1 && word.starts_with('-') {
            return Some(self.unsupported(word, offset));
        }
        let word = word.strip_prefix('+').unwrap_or(word);

        let Some((op, raw_value)) = word.split_once(':') else {
            return bare_word(word, at_end);
        };
        let op_lower = op.to_ascii_lowercase();
        let field = match op_lower.as_str() {
            "from" => Field::From,
            "to" => Field::To,
            "cc" => Field::Cc,
            "bcc" => Field::Bcc,
            "subject" => Field::Subject,
            "label" => Field::Label,
            "is" if raw_value.eq_ignore_ascii_case("list") => {
                return Some(Expr::IsList);
            }
            "is" => return Some(self.unsupported(word, offset)),
            other if GMAIL_ONLY_OPERATORS.contains(&other) => {
                return Some(self.unsupported(word, offset));
            }
            _ => return bare_word(word, at_end),
        };

        if raw_value.starts_with('(') {
            return Some(self.unsupported(word, offset));
        }
        let (value, closed_quote) = match raw_value.strip_prefix('"') {
            Some(rest) => match rest.strip_suffix('"') {
                Some(inner) => (inner, true),
                None if rest.contains('"') => (rest.split('"').next().unwrap_or(""), true),
                None => {
                    self.error(offset, "Missing closing quote");
                    (rest, false)
                }
            },
            None => (raw_value, false),
        };
        if value.trim().is_empty() {
            self.error(offset, format!("{op_lower}: needs a value"));
            return None;
        }
        Some(Expr::Term(Term {
            field,
            value: value.to_owned(),
            prefix: at_end && !closed_quote,
        }))
    }

    fn unsupported(&mut self, word: &str, offset: usize) -> Expr {
        self.error(offset, format!("{word} isn't supported in split rules"));
        Expr::Unsupported(word.to_owned())
    }
}

fn bare_word(word: &str, at_end: bool) -> Option<Expr> {
    if !word.chars().any(char::is_alphanumeric) {
        return None;
    }
    Some(Expr::Term(Term {
        field: Field::Any,
        value: word.to_owned(),
        prefix: at_end,
    }))
}

/// One item stays itself; several become an `And` (or `Or`), flattening
/// nested lists of the same kind.
fn combine(items: Vec<Expr>, is_and: bool) -> Option<Expr> {
    if items.len() <= 1 {
        return items.into_iter().next();
    }
    let mut flat = Vec::with_capacity(items.len());
    for item in items {
        match item {
            Expr::And(xs) if is_and => flat.extend(xs),
            Expr::Or(xs) if !is_and => flat.extend(xs),
            other => flat.push(other),
        }
    }
    Some(if is_and {
        Expr::And(flat)
    } else {
        Expr::Or(flat)
    })
}

// ---------------------------------------------------------------------------
// Gmail query

/// The raw query with `is:list` (and `-is:list`) terms removed, for Gmail,
/// which does not know that operator. `AND`/`OR` and parentheses left
/// dangling by the removal are dropped too. Returns `raw` unchanged when it
/// has no `is:list`.
pub fn strip_is_list(raw: &str) -> String {
    #[derive(PartialEq)]
    enum Kind {
        Open,
        Close,
        Op,
        Other,
    }
    let is_list = |t: &Tok| match &t.kind {
        TokKind::Word(w) => {
            let w = w.strip_prefix('-').unwrap_or(w);
            w.eq_ignore_ascii_case("is:list")
        }
        _ => false,
    };

    let toks = lex(raw);
    if !toks.iter().any(is_list) {
        return raw.to_owned();
    }
    let mut kept: Vec<(Kind, &Tok)> = toks
        .iter()
        .filter(|t| !is_list(t))
        .map(|t| {
            let kind = match t.kind {
                TokKind::LParen => Kind::Open,
                TokKind::RParen => Kind::Close,
                _ if t.is_keyword("OR") || t.is_keyword("AND") => Kind::Op,
                _ => Kind::Other,
            };
            (kind, t)
        })
        .collect();

    // Drop operators with nothing on one side, and empty groups, until stable.
    loop {
        let n = kept.len();
        let drop = (0..n).find_map(|i| {
            let prev = if i == 0 { None } else { Some(&kept[i - 1].0) };
            let next = kept.get(i + 1).map(|k| &k.0);
            match kept[i].0 {
                Kind::Op if matches!(prev, None | Some(Kind::Open | Kind::Op)) => Some((i, 1)),
                Kind::Op if matches!(next, None | Some(Kind::Close)) => Some((i, 1)),
                Kind::Open if next == Some(&Kind::Close) => Some((i, 2)),
                _ => None,
            }
        });
        match drop {
            Some((i, count)) => {
                kept.drain(i..i + count);
            }
            None => break,
        }
    }

    let mut out = String::new();
    let mut prev: Option<&(Kind, &Tok)> = None;
    for item in &kept {
        if let Some(p) = prev {
            let touching = p.1.end == item.1.start;
            if !(touching || p.0 == Kind::Open || item.0 == Kind::Close) {
                out.push(' ');
            }
        }
        out.push_str(&raw[item.1.start..item.1.end]);
        prev = Some(item);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> String {
        parse(s).to_string()
    }

    fn strict_err(s: &str) -> String {
        parse_strict(s).unwrap_err().message
    }

    // --- parsing --------------------------------------------------------

    #[test]
    fn empty_and_blank_input_is_the_empty_query() {
        assert_eq!(p(""), "(and)");
        assert_eq!(p("   \t "), "(and)");
        assert_eq!(strict_err(""), "Enter a query");
        assert_eq!(strict_err("   "), "Enter a query");
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
            let q = parse(input);
            let _ = q.to_fts5();
            let _ = parse_strict(input);
            let _ = strip_is_list(input);
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
        let err = parse_strict("(a OR").unwrap_err();
        assert_eq!((err.message.as_str(), err.offset), ("Missing )", 0));
    }

    #[test]
    fn strict_accepts_valid_rules() {
        for rule in [
            "is:list",
            "label:CATEGORY_PROMOTIONS",
            r#"from:alice OR (subject:"q3 plan" AND to:team)"#,
            "from:@example.com",
        ] {
            assert!(parse_strict(rule).is_ok(), "{rule}");
        }
    }

    // --- matching -------------------------------------------------------

    fn msg() -> MessageFields {
        MessageFields {
            from: Address::new("Alice Smith", "alice@example.com"),
            to: vec![Address::new("Bob", "bob@corp.io")],
            cc: vec![Address::new("", "carol@corp.io")],
            bcc: vec![Address::new("Dan", "dan@secret.org")],
            subject: "Q3 Plan: Budget review".into(),
            body: "Please read the attached draft.".into(),
            thread_user_labels: vec!["Work/Projects".into(), "Clients".into()],
            message_label_ids: vec!["INBOX".into(), "CATEGORY_UPDATES".into(), "Label_7".into()],
            is_list: false,
        }
    }

    fn m(q: &str, mode: Mode) -> bool {
        parse(q).matches(&msg(), mode)
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
        assert!(!m("label:CATEGORY_PROMOTIONS", Mode::Rule));
        assert!(!m("label:starred", Mode::Rule));
        // User label ids are not names.
        assert!(!m("label:Label_7", Mode::Rule));
    }

    #[test]
    fn is_list_operator() {
        assert!(!m("is:list", Mode::Rule));
        let mut list = msg();
        list.is_list = true;
        assert!(parse("is:list").matches(&list, Mode::Rule));
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
        let mut fields = msg();
        fields.from = Address::new("Ölaf Ärger", "olaf@exämple.de");
        fields.subject = "ПРИВЕТ мир".into();
        assert!(parse("from:ölaf").matches(&fields, Mode::Rule));
        assert!(parse("from:EXÄMPLE").matches(&fields, Mode::Rule));
        assert!(parse("привет").matches(&fields, Mode::Rule));
    }

    // --- FTS5 -----------------------------------------------------------

    fn fts(q: &str) -> Option<String> {
        parse(q).to_fts5()
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
        assert!(parse("label:x a").needs_post_filter());
        assert!(parse("a OR bcc:b").needs_post_filter());
        assert!(!parse("a has:attachment").needs_post_filter());
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
        assert!(!parse("").has_local_terms());
        assert!(!parse("has:attachment -x").has_local_terms());
        assert!(parse("has:attachment x").has_local_terms());
        assert!(parse("is:list").has_local_terms());
    }

    /// Every FTS5 string we produce must be accepted by SQLite and find the row.
    #[test]
    fn fts_output_is_valid_sqlite() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE VIRTUAL TABLE t USING fts5(from_text, to_text, cc_text, subject, body);
             INSERT INTO t VALUES ('Alice Smith alice@example.com', 'Bob bob@corp.io',
               'carol@corp.io', 'Q3 plan: budget', 'The e-mail said \"hello\" *loudly* NEAR AND OR');",
        )
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

    // --- strip_is_list ----------------------------------------------------

    #[test]
    fn strip_is_list_cases() {
        assert_eq!(strip_is_list("from:a b"), "from:a b");
        assert_eq!(strip_is_list("is:list"), "");
        assert_eq!(strip_is_list("IS:LIST from:a"), "from:a");
        assert_eq!(strip_is_list("from:a is:list b"), "from:a b");
        assert_eq!(strip_is_list("is:list OR from:a"), "from:a");
        assert_eq!(strip_is_list("from:a OR is:list"), "from:a");
        assert_eq!(strip_is_list("a OR is:list OR b"), "a OR b");
        assert_eq!(strip_is_list("(is:list) a"), "a");
        assert_eq!(strip_is_list("(is:list OR x) a"), "(x) a");
        assert_eq!(strip_is_list("-is:list a"), "a");
        assert_eq!(strip_is_list("\"is:list\" a"), "\"is:list\" a");
        assert_eq!(strip_is_list("is:listing a"), "is:listing a");
        assert_eq!(strip_is_list("has:attachment is:list"), "has:attachment");
    }
}
