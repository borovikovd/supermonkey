use chrono::{DateTime, Days, Months, NaiveDate, NaiveTime, TimeZone};

use super::{Expr, Field, Query, RuleError, Term};
use crate::domain::Millis;
use crate::domain::snooze::resolve_local;

const GMAIL_ONLY_OPS: &[&str] = &[
    "filename",
    "has",
    "size",
    "larger",
    "smaller",
    "rfc822msgid",
    "deliveredto",
    "list",
    "bcc",
    "around",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TokenKind {
    LParen,
    RParen,
    Minus,
    Word(String),
    Quoted(String),
}

#[derive(Debug, Clone)]
pub(super) struct Token {
    pub(super) kind: TokenKind,
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) unclosed: bool,
}

pub(super) fn lex(input: &str) -> Vec<Token> {
    let bytes = input.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        if bytes[i] == b'-' {
            let mut end = i;
            while bytes.get(end) == Some(&b'-') {
                end += 1;
            }
            if bytes
                .get(end)
                .is_none_or(|b| b.is_ascii_whitespace() || *b == b')')
            {
                i = end;
                continue;
            }
        }
        let (token, next) = lex_one(input, i);
        out.push(token);
        i = next;
    }
    out
}

fn lex_one(input: &str, start: usize) -> (Token, usize) {
    let bytes = input.as_bytes();
    let single = |kind| {
        (
            Token {
                kind,
                start,
                end: start + 1,
                unclosed: false,
            },
            start + 1,
        )
    };
    match bytes[start] {
        b'(' => single(TokenKind::LParen),
        b')' => single(TokenKind::RParen),
        b'-' => single(TokenKind::Minus),
        b'"' => {
            let (text, end, unclosed) = scan_quoted(input, start);
            (
                Token {
                    kind: TokenKind::Quoted(text),
                    start,
                    end,
                    unclosed,
                },
                end,
            )
        }
        _ => {
            let (word, end, unclosed) = scan_word(input, start);
            (
                Token {
                    kind: TokenKind::Word(word),
                    start,
                    end,
                    unclosed,
                },
                end,
            )
        }
    }
}

fn scan_quoted(input: &str, start: usize) -> (String, usize, bool) {
    let bytes = input.as_bytes();
    let mut i = start + 1;
    let mut out = String::new();
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() && bytes[i + 1] == b'"' {
            out.push('"');
            i += 2;
            continue;
        }
        if bytes[i] == b'"' {
            return (out, i + 1, false);
        }
        if let Some(ch) = input[i..].chars().next() {
            out.push(ch);
            i += ch.len_utf8();
        } else {
            break;
        }
    }
    (out, i, true)
}

fn scan_word(input: &str, start: usize) -> (String, usize, bool) {
    let bytes = input.as_bytes();
    let mut i = start;
    let mut unclosed = false;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_whitespace() || b == b')' {
            break;
        }
        if b == b'(' {
            if input[start..i].contains(':') {
                let (next, bad) = scan_balanced_parens(input, i);
                i = next;
                unclosed |= bad;
            }
            break;
        }
        if b == b'"' {
            let (_, next, bad) = scan_quoted(input, i);
            i = next;
            unclosed |= bad;
            continue;
        }
        i += 1;
    }
    (input[start..i].to_owned(), i, unclosed)
}

fn scan_balanced_parens(input: &str, start: usize) -> (usize, bool) {
    let bytes = input.as_bytes();
    let mut i = start;
    let mut depth = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return (i + 1, false);
                }
            }
            b'"' => {
                let (_, next, _) = scan_quoted(input, i);
                i = next;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    (i, true)
}

struct Parser<'a, Tz: TimeZone> {
    input: &'a str,
    tokens: Vec<Token>,
    pos: usize,
    rule_mode: bool,
    now: Option<&'a DateTime<Tz>>,
    errors: Vec<RuleError>,
}

pub(super) fn parse_lenient<Tz: TimeZone>(input: &str, now: Option<&DateTime<Tz>>) -> Query {
    let tokens = lex(input);
    let mut parser = Parser {
        input,
        tokens,
        pos: 0,
        rule_mode: false,
        now,
        errors: Vec::new(),
    };
    let expr = parser.parse_or();
    Query { expr }
}

pub(super) fn parse_rule(input: &str) -> Result<Query, RuleError> {
    let tokens = lex(input);
    let mut parser: Parser<'_, chrono::Utc> = Parser {
        input,
        tokens,
        pos: 0,
        rule_mode: true,
        now: None,
        errors: Vec::new(),
    };
    let expr = parser.parse_or();
    if let Some(tok) = parser.tokens.get(parser.pos) {
        parser.push_error(
            tok.start,
            format!("unexpected {}", &input[tok.start..tok.end]),
        );
    }
    if let Some(err) = parser.errors.into_iter().min_by_key(|e| e.offset) {
        return Err(err);
    }
    Ok(Query { expr })
}

impl<Tz: TimeZone> Parser<'_, Tz> {
    fn push_error(&mut self, offset: usize, message: String) {
        if self.rule_mode {
            self.errors.push(RuleError { offset, message });
        }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn is_word(&self, expected: &str) -> bool {
        matches!(
            self.peek(),
            Some(Token {
                kind: TokenKind::Word(w),
                ..
            }) if w == expected
        )
    }

    fn parse_or(&mut self) -> Expr {
        let mut branches = Vec::new();
        let first = self.parse_and();
        if matches!(&first, Expr::And(items) if items.is_empty())
            && self.is_word("OR")
            && let Some(tok) = self.peek()
        {
            self.push_error(tok.start, "OR needs a term before it".to_owned());
        }
        branches.push(first);
        while self.is_word("OR") {
            let or_start = self.peek().map_or(0, |t| t.start);
            self.pos += 1;
            if matches!(self.peek().map(|t| &t.kind), None | Some(TokenKind::RParen)) {
                self.push_error(or_start, "OR needs a term after it".to_owned());
                break;
            }
            if self.is_word("OR") {
                self.push_error(or_start, "OR needs a term after it".to_owned());
                continue;
            }
            branches.push(self.parse_and());
        }
        branches.retain(|b| !matches!(b, Expr::And(items) if items.is_empty()));
        match branches.len() {
            0 => Expr::And(Vec::new()),
            1 => branches.into_iter().next().unwrap_or(Expr::And(Vec::new())),
            _ => Expr::Or(branches),
        }
    }

    fn parse_and(&mut self) -> Expr {
        let mut items = Vec::new();
        while let Some(tok) = self.peek() {
            if matches!(tok.kind, TokenKind::RParen) || self.is_word("OR") {
                break;
            }
            if self.is_word("AND") {
                let start = tok.start;
                self.pos += 1;
                let right_missing =
                    matches!(self.peek().map(|t| &t.kind), None | Some(TokenKind::RParen))
                        || self.is_word("OR")
                        || self.is_word("AND");
                if items.is_empty() || right_missing {
                    self.push_error(start, "AND needs a term on both sides".to_owned());
                }
                continue;
            }
            if let Some(item) = self.parse_unary() {
                items.push(item);
            }
        }
        match items.len() {
            1 => items.into_iter().next().unwrap_or(Expr::And(Vec::new())),
            _ => Expr::And(items),
        }
    }

    fn parse_unary(&mut self) -> Option<Expr> {
        let tok = self.peek()?.clone();
        if matches!(tok.kind, TokenKind::Minus) {
            self.pos += 1;
            let end_before = self.peek().map_or(tok.end, |t| t.end);
            let inner = self.parse_unary();
            let raw = self.input.get(tok.start..end_before).unwrap_or("-");
            self.push_error(tok.start, format!("{raw} isn't supported in split rules"));
            return inner.map(|e| Expr::Not(Box::new(e)));
        }
        if matches!(tok.kind, TokenKind::LParen) {
            self.pos += 1;
            let inner = self.parse_or();
            if matches!(self.peek().map(|t| &t.kind), Some(TokenKind::RParen)) {
                self.pos += 1;
            } else {
                self.push_error(tok.start, "unclosed (".to_owned());
            }
            return Some(inner);
        }
        self.parse_atom()
    }

    fn parse_atom(&mut self) -> Option<Expr> {
        let tok = self.peek()?.clone();
        self.pos += 1;
        let is_last = self.pos == self.tokens.len();
        match tok.kind {
            TokenKind::Quoted(text) => {
                if tok.unclosed {
                    self.push_error(tok.start, "unclosed quote".to_owned());
                }
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    return None;
                }
                Some(Expr::Term(Term::Text {
                    field: Field::Any,
                    value: trimmed.to_owned(),
                    prefix: !self.rule_mode && tok.unclosed && is_last,
                }))
            }
            TokenKind::Word(ref word) => self.parse_word(word, &tok, is_last),
            TokenKind::LParen | TokenKind::RParen | TokenKind::Minus => None,
        }
    }

    fn parse_word(&mut self, word: &str, tok: &Token, is_last: bool) -> Option<Expr> {
        if let Some((op, val)) = word.split_once(':') {
            return self.parse_operator(op, val, tok, is_last);
        }
        Some(Expr::Term(Term::Text {
            field: Field::Any,
            value: word.to_owned(),
            prefix: !self.rule_mode && is_last,
        }))
    }

    #[expect(
        clippy::too_many_lines,
        reason = "dispatches every Gmail/Supermonkey query operator in one flat match"
    )]
    fn parse_operator(
        &mut self,
        op: &str,
        raw_val: &str,
        tok: &Token,
        is_last: bool,
    ) -> Option<Expr> {
        let (val, quoted) = unquote_value(raw_val);
        if raw_val.starts_with('(') {
            if self.rule_mode {
                self.push_error(
                    tok.start,
                    format!(
                        "{}: isn't supported in split rules",
                        op.to_ascii_lowercase()
                    ),
                );
            }
            return Some(Expr::Unsupported);
        }
        if val.is_empty() {
            if self.rule_mode {
                self.push_error(tok.start, format!("{op}: needs a value"));
                return None;
            }
            return None;
        }
        let op_lower = op.to_ascii_lowercase();
        match op_lower.as_str() {
            "from" => Some(field_or_me(
                Field::From,
                val,
                !self.rule_mode && !quoted && is_last,
            )),
            "to" => Some(field_or_me(
                Field::To,
                val,
                !self.rule_mode && !quoted && is_last,
            )),
            "cc" => Some(field_or_me(
                Field::Cc,
                val,
                !self.rule_mode && !quoted && is_last,
            )),
            "subject" => Some(Expr::Term(Term::Text {
                field: Field::Subject,
                value: val,
                prefix: !self.rule_mode && !quoted && is_last,
            })),
            "label" => Some(Expr::Term(Term::Label(val))),
            "is" => Some(self.parse_is(&val, tok)),
            "in" => Some(self.parse_in(&val, tok)),
            "has" => Some(self.parse_has(&val, tok)),
            "category" => Some(self.parse_category(&val, tok)),
            "after" | "before" | "newer_than" | "older_than" => {
                Some(self.parse_date_op(&op_lower, &val, tok))
            }
            other if GMAIL_ONLY_OPS.contains(&other) => {
                self.push_error(
                    tok.start,
                    format!("{other}: isn't supported in split rules"),
                );
                Some(Expr::Unsupported)
            }
            _ => Some(Expr::Term(Term::Text {
                field: Field::Any,
                value: format!("{op}:{val}"),
                prefix: !self.rule_mode && !quoted && is_last,
            })),
        }
    }

    fn parse_is(&mut self, val: &str, tok: &Token) -> Expr {
        if val.eq_ignore_ascii_case("list") {
            return Expr::Term(Term::IsList);
        }
        if self.rule_mode {
            self.push_error(
                tok.start,
                format!("is:{val} isn't supported in split rules"),
            );
            return Expr::Unsupported;
        }
        match val.to_ascii_lowercase().as_str() {
            "unread" => Expr::Term(Term::IsUnread),
            "read" => Expr::Term(Term::IsRead),
            "starred" => Expr::Term(Term::IsStarred),
            "snoozed" => Expr::Term(Term::IsSnoozed),
            _ => Expr::Unsupported,
        }
    }

    fn parse_in(&mut self, val: &str, tok: &Token) -> Expr {
        if self.rule_mode {
            self.push_error(
                tok.start,
                format!("in:{val} isn't supported in split rules"),
            );
            return Expr::Unsupported;
        }
        match val.to_ascii_lowercase().as_str() {
            "inbox" => Expr::Term(Term::InInbox),
            "sent" => Expr::Term(Term::InSent),
            "trash" => Expr::Term(Term::InTrash),
            "spam" => Expr::Term(Term::InSpam),
            "draft" | "drafts" => Expr::Term(Term::InDrafts),
            "anywhere" => Expr::Term(Term::InAnywhere),
            "snoozed" => Expr::Term(Term::IsSnoozed),
            _ => Expr::Term(Term::Label(val.to_owned())),
        }
    }

    fn parse_has(&mut self, val: &str, tok: &Token) -> Expr {
        if self.rule_mode {
            self.push_error(
                tok.start,
                format!("has:{val} isn't supported in split rules"),
            );
            return Expr::Unsupported;
        }
        if val.eq_ignore_ascii_case("attachment") {
            Expr::Term(Term::HasAttachment)
        } else {
            Expr::Unsupported
        }
    }

    fn parse_category(&mut self, val: &str, tok: &Token) -> Expr {
        if self.rule_mode {
            self.push_error(
                tok.start,
                format!("category:{val} isn't supported in split rules"),
            );
            return Expr::Unsupported;
        }
        Expr::Term(Term::Category(val.to_ascii_uppercase()))
    }

    fn parse_date_op(&mut self, op: &str, val: &str, tok: &Token) -> Expr {
        if self.rule_mode {
            self.push_error(tok.start, format!("{op}: isn't supported in split rules"));
            return Expr::Unsupported;
        }
        let Some(now) = self.now else {
            return Expr::Unsupported;
        };
        match op {
            "after" => {
                parse_date(val, now).map_or(Expr::Unsupported, |ms| Expr::Term(Term::After(ms)))
            }
            "before" => {
                parse_date(val, now).map_or(Expr::Unsupported, |ms| Expr::Term(Term::Before(ms)))
            }
            "newer_than" => {
                parse_relative(val, now).map_or(Expr::Unsupported, |ms| Expr::Term(Term::After(ms)))
            }
            "older_than" => parse_relative(val, now)
                .map_or(Expr::Unsupported, |ms| Expr::Term(Term::Before(ms))),
            _ => Expr::Unsupported,
        }
    }
}

fn field_or_me(field: Field, val: String, prefix: bool) -> Expr {
    if val.eq_ignore_ascii_case("me") {
        Expr::Term(Term::Me(field))
    } else {
        Expr::Term(Term::Text {
            field,
            value: val,
            prefix,
        })
    }
}

fn unquote_value(raw: &str) -> (String, bool) {
    if let Some(rest) = raw.strip_prefix('"') {
        let stripped = rest.strip_suffix('"').unwrap_or(rest);
        (stripped.replace("\\\"", "\""), true)
    } else {
        (raw.to_owned(), false)
    }
}

fn parse_date<Tz: TimeZone>(val: &str, now: &DateTime<Tz>) -> Option<Millis> {
    let mut parts = val.split(['/', '-']);
    let year: i32 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    let date = NaiveDate::from_ymd_opt(year, month, day)?;
    let naive = date.and_time(NaiveTime::MIN);
    let dt = resolve_local(&now.timezone(), naive)?;
    Some(Millis(dt.timestamp_millis()))
}

fn parse_relative<Tz: TimeZone>(val: &str, now: &DateTime<Tz>) -> Option<Millis> {
    let trimmed = val.trim();
    let unit = trimmed.chars().last()?;
    let count: u32 = trimmed.strip_suffix(unit)?.parse().ok()?;
    let target = match unit.to_ascii_lowercase() {
        'd' => now.clone().checked_sub_days(Days::new(u64::from(count)))?,
        'm' => now.clone().checked_sub_months(Months::new(count))?,
        'y' => now
            .clone()
            .checked_sub_months(Months::new(count.checked_mul(12)?))?,
        _ => return None,
    };
    Some(Millis(target.timestamp_millis()))
}
