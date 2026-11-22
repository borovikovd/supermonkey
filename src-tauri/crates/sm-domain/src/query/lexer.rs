#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TokKind<'a> {
    LParen,
    RParen,
    Minus,
    Phrase { text: &'a str, closed: bool },
    Word(&'a str),
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Tok<'a> {
    pub(super) kind: TokKind<'a>,
    pub(super) start: usize,
    pub(super) end: usize,
}

impl Tok<'_> {
    pub(super) const fn is_keyword(&self, keyword: &str) -> bool {
        matches!(self.kind, TokKind::Word(w) if w.eq_ignore_ascii_case(keyword))
    }
}

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

fn scan_quote(s: &str, start: usize) -> (usize, bool) {
    match s[start + 1..].find('"') {
        Some(j) => (start + 1 + j + 1, true),
        None => (s.len(), false),
    }
}

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
