//! A recursive-descent parser that always produces a query, and lists the
//! problems a strict parse would refuse.

use super::lexer::{Tok, TokKind, lex};
use super::{Expr, Field, Query, RuleError, Term};

/// Gmail operators this language does not evaluate. A word starting with one
/// of these and `:` is [`Expr::Unsupported`]. Any other `word:word` (like
/// `re:budget` or `10:30`) is a plain bare word.
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

/// The best-effort query for `input`, and every problem found on the way.
pub(super) fn parse(input: &str) -> (Query, Vec<RuleError>) {
    let mut parser = Parser {
        input_len: input.len(),
        toks: lex(input),
        pos: 0,
        errors: Vec::new(),
    };
    let expr = parser.or_expr(0).unwrap_or(Expr::And(Vec::new()));
    (Query::from_expr(expr), parser.errors)
}

struct Parser<'a> {
    input_len: usize,
    toks: Vec<Tok<'a>>,
    pos: usize,
    errors: Vec<RuleError>,
}

impl<'a> Parser<'a> {
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
                TokKind::LParen => {
                    self.pos += 1;
                    let inner = self.or_expr(depth + 1);
                    if self.peek().is_some_and(|t| t.kind == TokKind::RParen) {
                        self.pos += 1;
                    } else {
                        self.errors
                            .push(RuleError::MissingParen { offset: tok.start });
                    }
                    match inner {
                        Some(e) => items.push(e),
                        None => self
                            .errors
                            .push(RuleError::EmptyParens { offset: tok.start }),
                    }
                }
                TokKind::Phrase { text, closed } => {
                    self.pos += 1;
                    if !closed {
                        self.errors
                            .push(RuleError::MissingQuote { offset: tok.start });
                    }
                    if text.chars().any(char::is_alphanumeric) {
                        items.push(Expr::Term(Term {
                            field: Field::Any,
                            value: text.to_owned(),
                            prefix: !closed,
                        }));
                    }
                }
                TokKind::Word(word) => {
                    self.pos += 1;
                    let at_end = tok.end == self.input_len;
                    items.extend(self.word(word, tok.start, at_end));
                }
            }
        }
        combine(items, true)
    }

    /// What one word token means: `None` when there is nothing to match on,
    /// such as `from:` with no value or a lone `&`.
    fn word(&mut self, word: &str, offset: usize, at_end: bool) -> Option<Expr> {
        if word.len() > 1 && word.starts_with('-') {
            return Some(self.unsupported(word, offset));
        }
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
            "is" if raw_value.eq_ignore_ascii_case("list") => return Some(Expr::IsList),
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
                    self.errors.push(RuleError::MissingQuote { offset });
                    (rest, false)
                }
            },
            None => (raw_value, false),
        };
        if value.trim().is_empty() {
            self.errors
                .push(RuleError::MissingValue { offset, operator });
            return None;
        }
        Some(Expr::Term(Term {
            field,
            value: value.to_owned(),
            prefix: at_end && !closed_quote,
        }))
    }

    fn unsupported(&mut self, word: &str, offset: usize) -> Expr {
        self.errors.push(RuleError::Unsupported {
            offset,
            term: word.to_owned(),
        });
        Expr::Unsupported(word.to_owned())
    }
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
