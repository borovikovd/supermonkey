//! The query as Gmail should get it.

use super::lexer::{Tok, TokKind, lex};

/// How a kept token joins its neighbours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Open,
    Close,
    Op,
    Other,
}

/// `raw` with its `is:list` (and `-is:list`) terms removed, for Gmail, which
/// does not know that operator. `AND`, `OR`, and parentheses the removal
/// leaves dangling go too. `raw` comes back unchanged when it has no `is:list`.
pub fn gmail_query(raw: &str) -> String {
    let toks = lex(raw);
    if !toks.iter().any(is_list) {
        return raw.to_owned();
    }
    let mut kept: Vec<(Kind, Tok<'_>)> = toks
        .into_iter()
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
    while let Some((i, count)) = dangling(&kept) {
        kept.drain(i..i + count);
    }

    let mut out = String::new();
    let mut prev: Option<&(Kind, Tok<'_>)> = None;
    for item in &kept {
        if let Some((prev_kind, prev_tok)) = prev {
            let touching = prev_tok.end == item.1.start;
            if !(touching || *prev_kind == Kind::Open || item.0 == Kind::Close) {
                out.push(' ');
            }
        }
        out.push_str(&raw[item.1.start..item.1.end]);
        prev = Some(item);
    }
    out
}

fn is_list(t: &Tok<'_>) -> bool {
    match t.kind {
        TokKind::Word(w) => w
            .strip_prefix('-')
            .unwrap_or(w)
            .eq_ignore_ascii_case("is:list"),
        _ => false,
    }
}

/// The first dangling operator or empty group: its index and length.
fn dangling(kept: &[(Kind, Tok<'_>)]) -> Option<(usize, usize)> {
    (0..kept.len()).find_map(|i| {
        let prev = i.checked_sub(1).and_then(|p| kept.get(p)).map(|k| k.0);
        let next = kept.get(i + 1).map(|k| k.0);
        match kept.get(i)?.0 {
            Kind::Op if matches!(prev, None | Some(Kind::Open | Kind::Op)) => Some((i, 1)),
            Kind::Op if matches!(next, None | Some(Kind::Close)) => Some((i, 1)),
            Kind::Open if next == Some(Kind::Close) => Some((i, 2)),
            _ => None,
        }
    })
}
