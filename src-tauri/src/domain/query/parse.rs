use std::borrow::Cow;
use std::collections::VecDeque;

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

#[derive(Debug, PartialEq, Eq)]
pub(super) enum TokenKind<'a> {
    LParen,
    RParen,
    Minus,
    Word(&'a str),
    Quoted(Cow<'a, str>),
}

#[derive(Debug)]
pub(super) struct Token<'a> {
    pub(super) kind: TokenKind<'a>,
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) unclosed: bool,
}

pub(super) fn lex(input: &str) -> Vec<Token<'_>> {
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

fn lex_one(input: &str, start: usize) -> (Token<'_>, usize) {
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

fn scan_quoted(input: &str, start: usize) -> (Cow<'_, str>, usize, bool) {
    let bytes = input.as_bytes();
    let mut i = start + 1;
    let mut owned: Option<String> = None;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() && bytes[i + 1] == b'"' {
            let buf = owned.get_or_insert_with(|| input[start + 1..i].to_owned());
            buf.push('"');
            i += 2;
            continue;
        }
        if bytes[i] == b'"' {
            let text = owned.map_or_else(|| Cow::Borrowed(&input[start + 1..i]), Cow::Owned);
            return (text, i + 1, false);
        }
        if let Some(ch) = input[i..].chars().next() {
            if let Some(buf) = &mut owned {
                buf.push(ch);
            }
            i += ch.len_utf8();
        } else {
            break;
        }
    }
    let text = owned.map_or_else(|| Cow::Borrowed(&input[start + 1..i]), Cow::Owned);
    (text, i, true)
}

fn scan_word(input: &str, start: usize) -> (&str, usize, bool) {
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
    (&input[start..i], i, unclosed)
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
    tokens: VecDeque<Token<'a>>,
    rule_mode: bool,
    now: Option<&'a DateTime<Tz>>,
    errors: Vec<RuleError>,
}

pub(super) fn parse_lenient<Tz: TimeZone>(input: &str, now: Option<&DateTime<Tz>>) -> Query {
    let mut parser = Parser {
        input,
        tokens: VecDeque::from(lex(input)),
        rule_mode: false,
        now,
        errors: Vec::new(),
    };
    let expr = parser.parse_or();
    Query { expr }
}

pub(super) fn parse_rule(input: &str) -> Result<Query, RuleError> {
    let mut parser: Parser<'_, chrono::Utc> = Parser {
        input,
        tokens: VecDeque::from(lex(input)),
        rule_mode: true,
        now: None,
        errors: Vec::new(),
    };
    let expr = parser.parse_or();
    if let Some(tok) = parser.peek() {
        let (start, end) = (tok.start, tok.end);
        parser.push_error(start, || format!("unexpected {}", &input[start..end]));
    }
    if let Some(err) = parser.errors.into_iter().min_by_key(|e| e.offset) {
        return Err(err);
    }
    Ok(Query { expr })
}

impl<'a, Tz: TimeZone> Parser<'a, Tz> {
    fn push_error(&mut self, offset: usize, make: impl FnOnce() -> String) {
        if self.rule_mode {
            self.errors.push(RuleError {
                offset,
                message: make(),
            });
        }
    }

    fn push_static_error(&mut self, offset: usize, message: &'static str) {
        if self.rule_mode {
            self.errors.push(RuleError {
                offset,
                message: message.to_owned(),
            });
        }
    }

    fn peek(&self) -> Option<&Token<'a>> {
        self.tokens.front()
    }

    fn is_word(&self, expected: &str) -> bool {
        matches!(
            self.peek(),
            Some(Token {
                kind: TokenKind::Word(w),
                ..
            }) if *w == expected
        )
    }

    fn parse_or(&mut self) -> Expr {
        let mut branches = Vec::new();
        let first = self.parse_and();
        if matches!(&first, Expr::And(items) if items.is_empty())
            && self.is_word("OR")
            && let Some(tok) = self.peek()
        {
            let start = tok.start;
            self.push_static_error(start, "OR needs a term before it");
        }
        branches.push(first);
        while self.is_word("OR") {
            let or_start = self.tokens.pop_front().map_or(0, |t| t.start);
            if matches!(self.peek().map(|t| &t.kind), None | Some(TokenKind::RParen)) {
                self.push_static_error(or_start, "OR needs a term after it");
                break;
            }
            if self.is_word("OR") {
                self.push_static_error(or_start, "OR needs a term after it");
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
                self.tokens.pop_front();
                let right_missing =
                    matches!(self.peek().map(|t| &t.kind), None | Some(TokenKind::RParen))
                        || self.is_word("OR")
                        || self.is_word("AND");
                if items.is_empty() || right_missing {
                    self.push_static_error(start, "AND needs a term on both sides");
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
        match self.peek()? {
            Token {
                kind: TokenKind::Minus,
                start,
                end,
                ..
            } => {
                let (start, end) = (*start, *end);
                self.tokens.pop_front();
                let end_before = self.peek().map_or(end, |t| t.end);
                let inner = self.parse_unary();
                let raw = self.input.get(start..end_before).unwrap_or("-");
                self.push_error(start, || format!("{raw} isn't supported in split rules"));
                inner.map(|e| Expr::Not(Box::new(e)))
            }
            Token {
                kind: TokenKind::LParen,
                start,
                ..
            } => {
                let open = *start;
                self.tokens.pop_front();
                let inner = self.parse_or();
                if matches!(self.peek().map(|t| &t.kind), Some(TokenKind::RParen)) {
                    self.tokens.pop_front();
                } else {
                    self.push_static_error(open, "unclosed (");
                }
                Some(inner)
            }
            _ => self.parse_atom(),
        }
    }

    fn parse_atom(&mut self) -> Option<Expr> {
        let tok = self.tokens.pop_front()?;
        let is_last = self.tokens.is_empty();
        match tok.kind {
            TokenKind::Quoted(text) => {
                if tok.unclosed {
                    self.push_static_error(tok.start, "unclosed quote");
                }
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    return None;
                }
                let value = match text {
                    Cow::Owned(s) if trimmed.len() == s.len() => s,
                    Cow::Borrowed(_) | Cow::Owned(_) => trimmed.to_owned(),
                };
                Some(Expr::Term(Term::Text {
                    field: Field::Any,
                    value,
                    prefix: !self.rule_mode && tok.unclosed && is_last,
                }))
            }
            TokenKind::Word(word) => self.parse_word(word, tok.start, is_last),
            TokenKind::LParen | TokenKind::RParen | TokenKind::Minus => None,
        }
    }

    fn parse_word(&mut self, word: &str, start: usize, is_last: bool) -> Option<Expr> {
        if let Some((op, val)) = word.split_once(':') {
            return self.parse_operator(op, val, start, is_last);
        }
        Some(Expr::Term(Term::Text {
            field: Field::Any,
            value: word.to_owned(),
            prefix: !self.rule_mode && is_last,
        }))
    }

    fn parse_operator(
        &mut self,
        op: &str,
        raw_val: &str,
        start: usize,
        is_last: bool,
    ) -> Option<Expr> {
        if raw_val.starts_with('(') {
            self.push_error(start, || {
                format!(
                    "{}: isn't supported in split rules",
                    op.to_ascii_lowercase()
                )
            });
            return Some(Expr::Unsupported);
        }
        let (val, quoted) = unquote_value(raw_val);
        if val.is_empty() {
            self.push_error(start, || format!("{op}: needs a value"));
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
                value: val.into_owned(),
                prefix: !self.rule_mode && !quoted && is_last,
            })),
            "label" => Some(Expr::Term(Term::Label(val.into_owned()))),
            "is" => Some(self.parse_is(&val, start)),
            "in" => Some(self.parse_in(val, start)),
            "has" => Some(self.parse_has(&val, start)),
            "category" => Some(self.parse_category(&val, start)),
            "after" | "before" | "newer_than" | "older_than" => {
                Some(self.parse_date_op(&op_lower, &val, start))
            }
            other if GMAIL_ONLY_OPS.contains(&other) => {
                self.push_error(start, || format!("{other}: isn't supported in split rules"));
                Some(Expr::Unsupported)
            }
            _ => Some(Expr::Term(Term::Text {
                field: Field::Any,
                value: format!("{op}:{val}"),
                prefix: !self.rule_mode && !quoted && is_last,
            })),
        }
    }

    fn parse_is(&mut self, val: &str, start: usize) -> Expr {
        if val.eq_ignore_ascii_case("list") {
            return Expr::Term(Term::IsList);
        }
        if self.rule_mode {
            self.push_error(start, || format!("is:{val} isn't supported in split rules"));
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

    fn parse_in(&mut self, val: Cow<'_, str>, start: usize) -> Expr {
        if self.rule_mode {
            self.push_error(start, || format!("in:{val} isn't supported in split rules"));
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
            _ => Expr::Term(Term::Label(val.into_owned())),
        }
    }

    fn parse_has(&mut self, val: &str, start: usize) -> Expr {
        if self.rule_mode {
            self.push_error(start, || {
                format!("has:{val} isn't supported in split rules")
            });
            return Expr::Unsupported;
        }
        if val.eq_ignore_ascii_case("attachment") {
            Expr::Term(Term::HasAttachment)
        } else {
            Expr::Unsupported
        }
    }

    fn parse_category(&mut self, val: &str, start: usize) -> Expr {
        if self.rule_mode {
            self.push_error(start, || {
                format!("category:{val} isn't supported in split rules")
            });
            return Expr::Unsupported;
        }
        Expr::Term(Term::Category(val.to_ascii_uppercase()))
    }

    fn parse_date_op(&mut self, op: &str, val: &str, start: usize) -> Expr {
        if self.rule_mode {
            self.push_error(start, || format!("{op}: isn't supported in split rules"));
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

fn field_or_me(field: Field, val: Cow<'_, str>, prefix: bool) -> Expr {
    if val.eq_ignore_ascii_case("me") {
        Expr::Term(Term::Me(field))
    } else {
        Expr::Term(Term::Text {
            field,
            value: val.into_owned(),
            prefix,
        })
    }
}

fn unquote_value(raw: &str) -> (Cow<'_, str>, bool) {
    if let Some(rest) = raw.strip_prefix('"') {
        let stripped = rest.strip_suffix('"').unwrap_or(rest);
        if stripped.contains("\\\"") {
            (Cow::Owned(stripped.replace("\\\"", "\"")), true)
        } else {
            (Cow::Borrowed(stripped), true)
        }
    } else {
        (Cow::Borrowed(raw), false)
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
