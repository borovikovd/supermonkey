//! A fresh composer's From, To, and Cc (SPEC Addressing).

use std::collections::HashSet;

use crate::{Addr, DraftKind, Identity};

/// The headers of the message a reply or forward is aimed at.
#[derive(Debug, Clone, Copy)]
pub struct ReplyTarget<'a> {
    /// Its `From`.
    pub from: &'a Addr,
    /// Its `To`.
    pub to: &'a [Addr],
    /// Its `Cc`.
    pub cc: &'a [Addr],
    /// Its `Reply-To`, empty when it has none.
    pub reply_to: &'a [Addr],
}

/// A composer's starting From, To, and Cc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Addressing {
    /// The `sendAs` address to send from.
    pub from: Addr,
    /// The `To` mailboxes.
    pub to: Vec<Addr>,
    /// The `Cc` mailboxes.
    pub cc: Vec<Addr>,
}

/// The addressing a fresh composer of `kind` starts with. New mail and
/// forwards send from the default with no recipients; replies follow
/// `reply_addressing`. A reply with no target starts like new mail.
pub fn addressing(
    kind: DraftKind,
    target: Option<ReplyTarget<'_>>,
    me: &Identity,
    reply_all: bool,
) -> Addressing {
    match (kind, target) {
        (DraftKind::Reply, Some(target)) => reply_addressing(target, me, reply_all),
        _ => Addressing {
            from: me.default_from(),
            to: Vec::new(),
            cc: Vec::new(),
        },
    }
}

/// Reply and reply all to `target` (SPEC Addressing).
///
/// - Mail this account sent: send from its From, to its To; reply all puts
///   its Cc on Cc, minus this account's addresses.
/// - Otherwise: send from the `sendAs` address it was sent to (To before Cc,
///   the first one within a field), else the default. To is its Reply-To,
///   or its From when it has none. Reply all puts its other To and Cc on
///   Cc, minus this account's addresses and anyone already on To.
///
/// Cc never repeats an address (ignoring case); the first one is kept.
pub(crate) fn reply_addressing(
    target: ReplyTarget<'_>,
    me: &Identity,
    reply_all: bool,
) -> Addressing {
    if me.is_mine(&target.from.email) {
        let cc = if reply_all {
            distinct(target.cc, me, HashSet::new())
        } else {
            Vec::new()
        };
        return Addressing {
            from: target.from.clone(),
            to: target.to.to_vec(),
            cc,
        };
    }
    let from = target
        .to
        .iter()
        .chain(target.cc)
        .find_map(|a| me.send_as_for(&a.email))
        .map_or_else(|| me.default_from(), crate::SendAsAddress::addr);
    let to = if target.reply_to.is_empty() {
        vec![target.from.clone()]
    } else {
        target.reply_to.to_vec()
    };
    let cc = if reply_all {
        let on_to = to.iter().map(Addr::key).collect();
        distinct(target.to.iter().chain(target.cc), me, on_to)
    } else {
        Vec::new()
    };
    Addressing { from, to, cc }
}

/// The addresses that are not this account's and not in `seen`, first
/// occurrence kept.
fn distinct<'a>(
    list: impl IntoIterator<Item = &'a Addr>,
    me: &Identity,
    mut seen: HashSet<String>,
) -> Vec<Addr> {
    list.into_iter()
        .filter(|a| !me.is_mine(&a.email) && seen.insert(a.key()))
        .cloned()
        .collect()
}
