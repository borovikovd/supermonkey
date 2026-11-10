//! Splits a query into tokens that borrow the input.

/// What a token is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TokKind<'a> {
    /// `(`.
    LParen,
    /// `)`.
    RParen,
    /// A `-` right before a term, which it negates.
    Minus,
    /// A bare `"quoted phrase"`.
    Phrase { text: &'a str, closed: bool },
    /// Anything else, including `op:value` and `op:"quoted value"`.
    Word(&'a str),
}

/// A token and the byte range it covers.
#[derive(Debug, Clone, Copy)]
pub(super) struct Tok<'a> {
    pub(super) kind: TokKind<'a>,
    pub(super) start: usize,
    pub(super) end: usize,
}

impl Tok<'_> {
    /// Whether this is the bare word `keyword` (`AND`, `OR`), ignoring case.
    pub(super) const fn is_keyword(&self, keyword: &str) -> bool {
        matches!(self.kind, TokKind::Word(w) if w.eq_ignore_ascii_case(keyword))
    }
}

/// The tokens of `s`, in order.
pub(super) fn lex(s: &str) -> Vec<Tok<'_>> {
    let mut toks = Vec::new();
    let mut i = 0;
    while let Some(c) = s[i..].chars().next() {
        let start = i;
        let kind = match c {
            _ if c.is_whitespace() => {
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
            '-' if s[i + 1..]
                .chars()
                .next()
                .is_some_and(|next| !next.is_whitespace() && next != ')') =>
            {
                i += 1;
                TokKind::Minus
            }
            '"' => {
                let (end, closed) = scan_quote(s, start);
                i = end;
                let inner_end = if closed { end - 1 } else { end };
                TokKind::Phrase {
                    text: &s[start + 1..inner_end],
                    closed,
                }
            }
            _ => {
                i = scan_word(s, start);
                TokKind::Word(&s[start..i])
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

/// Index just past the closing quote of the quote at `start`, and whether
/// there was one.
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
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return start + i + 1;
                }
            }
            _ => {}
        }
    }
    s.len()
}

/// Index just past the word at `start`. A quote or group right after `op:`
/// belongs to the word.
fn scan_word(s: &str, start: usize) -> usize {
    let mut i = start;
    while let Some(c) = s[i..].chars().next() {
        let attached = s[..i].ends_with(':');
        match c {
            '"' if attached => i = scan_quote(s, i).0,
            '(' if attached => i = scan_group(s, i),
            _ if c.is_whitespace() || c == '(' || c == ')' => break,
            _ => i += c.len_utf8(),
        }
    }
    i
}
