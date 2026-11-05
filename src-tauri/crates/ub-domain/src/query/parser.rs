//! A recursive-descent parser that always produces a query, and lists the
//! problems a strict parse would refuse.

use super::dates::{DateArg, age, day_or_epoch};
use super::lexer::{Tok, TokKind, lex};
use super::{Expr, Field, Query, RuleError, Term};
use crate::{Millis, SystemLabel};

/// Gmail operators this language does not evaluate. A word starting with one
/// of these and `:` is [`Expr::Unsupported`]. Any other `word:word` (like
/// `re:budget` or `10:30`) is a plain bare word.
const GMAIL_ONLY_OPERATORS: &[&str] = &[
    "filename",
    "larger",
    "smaller",
    "size",
    "list",
    "deliveredto",
    "rfc822msgid",
    "around",
];

/// Places a date operator's value in time, or `None` when it can't.
pub(super) type PlaceDate<'p> = dyn Fn(&DateArg) -> Option<Millis> + 'p;

/// The best-effort query for `input`, and every problem found on the way.
pub(super) fn parse(input: &str, place_date: &PlaceDate<'_>) -> (Query, Vec<RuleError>) {
    let mut parser = Parser {
        input,
        toks: lex(input),
        pos: 0,
        errors: Vec::new(),
        place_date,
    };
    let expr = parser.or_expr(0).unwrap_or(Expr::And(Vec::new()));
    (Query::from_expr(expr), parser.errors)
}

struct Parser<'a, 'p> {
    input: &'a str,
    toks: Vec<Tok<'a>>,
    pos: usize,
    errors: Vec<RuleError>,
    place_date: &'p PlaceDate<'p>,
}

impl<'a> Parser<'a, '_> {
    fn peek(&self) -> Option<Tok<'a>> {
        self.toks.get(self.pos).copied()
    }

    /// `and_expr (OR and_expr)*`
    fn or_expr(&mut self, depth: usize) -> Option<Expr> {
        let mut alternatives = Vec::new();
        let first = self.and_expr(depth);
        if first.is_none()
            && let Some(tok) = self.peek().filter(|t| t.is_keyword("OR"))
        {
            self.errors
                .push(RuleError::OrWithoutLeft { offset: tok.start });
        }
        alternatives.extend(first);
        while let Some(tok) = self.peek().filter(|t| t.is_keyword("OR")) {
            self.pos += 1;
            match self.and_expr(depth) {
                Some(e) => alternatives.push(e),
                None => self
                    .errors
                    .push(RuleError::OrWithoutRight { offset: tok.start }),
            }
        }
        combine(alternatives, false)
    }

    /// `unary ((AND)? unary)*`, stopping at OR, a closing parenthesis inside
    /// a group, or the end.
    fn and_expr(&mut self, depth: usize) -> Option<Expr> {
        let mut items = Vec::new();
        while let Some(tok) = self.peek() {
            if tok.is_keyword("OR") {
                break;
            }
            if tok.is_keyword("AND") {
                self.pos += 1;
                let right_missing = self
                    .peek()
                    .is_none_or(|next| next.is_keyword("OR") || next.kind == TokKind::RParen);
                if items.is_empty() || right_missing {
                    self.errors
                        .push(RuleError::AndWithoutBoth { offset: tok.start });
                }
                continue;
            }
            match tok.kind {
                TokKind::RParen if depth > 0 => break,
                TokKind::RParen => {
                    self.pos += 1;
                    self.errors
                        .push(RuleError::UnmatchedParen { offset: tok.start });
                }
                _ => items.extend(self.unary(depth)),
            }
        }
        combine(items, true)
    }

    /// One term at the current token, which is not `)`: a group, a phrase,
    /// a word, or `-` and the term it negates. `None` when there is nothing
    /// to match on, such as `from:` with no value or a lone `&`.
    fn unary(&mut self, depth: usize) -> Option<Expr> {
        let tok = self.peek()?;
        self.pos += 1;
        match tok.kind {
            TokKind::Minus => {
                // `-` is never last and never before `)`: the lexer makes
                // those a word.
                let negated = self.unary(depth)?;
                let end = self.toks.get(self.pos - 1).map_or(tok.end, |t| t.end);
                let input = self.input;
                self.search_only(&input[tok.start..end], tok.start);
                Some(Expr::Not(Box::new(negated)))
            }
            TokKind::LParen => {
                let inner = self.or_expr(depth + 1);
                if self.peek().is_some_and(|t| t.kind == TokKind::RParen) {
                    self.pos += 1;
                } else {
                    self.errors
                        .push(RuleError::MissingParen { offset: tok.start });
                }
                if inner.is_none() {
                    self.errors
                        .push(RuleError::EmptyParens { offset: tok.start });
                }
                inner
            }
            TokKind::Phrase { text, closed } => {
                if !closed {
                    self.errors
                        .push(RuleError::MissingQuote { offset: tok.start });
                }
                text.chars().any(char::is_alphanumeric).then(|| {
                    Expr::Term(Term {
                        field: Field::Any,
                        value: text.to_owned(),
                        prefix: !closed,
                    })
                })
            }
            TokKind::Word(word) => {
                let at_end = tok.end == self.input.len();
                self.word(word, tok.start, at_end)
            }
            TokKind::RParen => None,
        }
    }

    /// What one word token means.
    fn word(&mut self, word: &str, offset: usize, at_end: bool) -> Option<Expr> {
        let word = word.strip_prefix('+').unwrap_or(word);
        let Some((op, raw_value)) = word.split_once(':') else {
            return bare_word(word, at_end);
        };
        let operator = op.to_ascii_lowercase();
        let field = match operator.as_str() {
            "from" => Field::From,
            "to" => Field::To,
            "cc" => Field::Cc,
            "bcc" => Field::Bcc,
            "subject" => Field::Subject,
            "label" => Field::Label,
            "is" | "in" | "has" | "category" | "after" | "before" | "newer" | "older"
            | "newer_than" | "older_than" => {
                return self.search_operator(word, &operator, raw_value, offset);
            }
            other if GMAIL_ONLY_OPERATORS.contains(&other) => {
                return Some(self.unsupported(word, offset));
            }
            _ => return bare_word(word, at_end),
        };
        if raw_value.starts_with('(') {
            return Some(self.unsupported(word, offset));
        }
        let (value, closed_quote) = self.value(raw_value, offset, &operator)?;
        Some(Expr::Term(Term {
            field,
            value: value.to_owned(),
            prefix: at_end && !closed_quote,
        }))
    }

    /// An operator's value without its quotes, and whether it had a closing
    /// one. `None`, noted, when it is blank.
    fn value<'v>(
        &mut self,
        raw_value: &'v str,
        offset: usize,
        operator: &str,
    ) -> Option<(&'v str, bool)> {
        let (value, closed_quote) = match raw_value.strip_prefix('"') {
            Some(rest) => match rest.strip_suffix('"') {
                Some(inner) => (inner, true),
                None if rest.contains('"') => (rest.split('"').next().unwrap_or(""), true),
                None => {
                    self.errors.push(RuleError::MissingQuote { offset });
                    (rest, false)
                }
            },
            None => (raw_value, false),
        };
        if value.trim().is_empty() {
            self.errors.push(RuleError::MissingValue {
                offset,
                operator: operator.to_owned(),
            });
            return None;
        }
        Some((value, closed_quote))
    }

    /// `is:`, `in:`, `has:`, `category:`, and the date operators, which
    /// search evaluates and split rules refuse (`is:list` aside).
    fn search_operator(
        &mut self,
        word: &str,
        operator: &str,
        raw_value: &str,
        offset: usize,
    ) -> Option<Expr> {
        if raw_value.starts_with('(') {
            return Some(self.unsupported(word, offset));
        }
        let (value, _) = self.value(raw_value, offset, operator)?;
        let value = value.trim();
        let lower = value.to_lowercase();
        if operator == "is" && lower == "list" {
            return Some(Expr::IsList);
        }
        let label = |label: SystemLabel| Some(label_term(label.id()));
        let expr = match (operator, lower.as_str()) {
            ("is", "unread") => label(SystemLabel::Unread),
            ("is", "read") => label(SystemLabel::Unread).map(|e| Expr::Not(Box::new(e))),
            ("is", "starred") => label(SystemLabel::Starred),
            ("is", "important") => label(SystemLabel::Important),
            ("is" | "in", "snoozed") => Some(Expr::Snoozed),
            ("in", "anywhere") => Some(Expr::Anywhere),
            ("in", "draft" | "drafts") => label(SystemLabel::Draft),
            ("in", "chats") => label(SystemLabel::Chat),
            // Gmail reads `in:` as `label:`: a system label or a user label name.
            ("in", _) => Some(label_term(value)),
            ("has", "attachment") => Some(Expr::HasAttachment),
            ("category", "primary") => label(SystemLabel::CategoryPersonal),
            ("category", "social") => label(SystemLabel::CategorySocial),
            ("category", "promotions") => label(SystemLabel::CategoryPromotions),
            ("category", "updates") => label(SystemLabel::CategoryUpdates),
            ("category", "forums") => label(SystemLabel::CategoryForums),
            ("after" | "newer" | "before" | "older", _) => {
                self.date(matches!(operator, "after" | "newer"), day_or_epoch(value))
            }
            ("newer_than" | "older_than", _) => self.date(operator == "newer_than", age(value)),
            _ => None,
        };
        Some(match expr {
            Some(expr) => {
                self.search_only(word, offset);
                expr
            }
            None => self.unsupported(word, offset),
        })
    }

    /// A date test, or `None` when the value is no date or can't be placed
    /// in time.
    fn date(&self, after: bool, arg: Option<DateArg>) -> Option<Expr> {
        let at = (self.place_date)(&arg?)?;
        Some(Expr::Date { after, at })
    }

    /// Notes a term that search evaluates and a split rule refuses.
    fn search_only(&mut self, term: &str, offset: usize) {
        self.errors.push(RuleError::Unsupported {
            offset,
            term: term.to_owned(),
        });
    }

    fn unsupported(&mut self, word: &str, offset: usize) -> Expr {
        self.search_only(word, offset);
        Expr::Unsupported(word.to_owned())
    }
}

/// A `label:` test for a system id or a user label name.
fn label_term(value: &str) -> Expr {
    Expr::Term(Term {
        field: Field::Label,
        value: value.to_owned(),
        prefix: false,
    })
}

fn bare_word(word: &str, at_end: bool) -> Option<Expr> {
    word.chars().any(char::is_alphanumeric).then(|| {
        Expr::Term(Term {
            field: Field::Any,
            value: word.to_owned(),
            prefix: at_end,
        })
    })
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
