use std::collections::HashSet;

use crate::{Addr, DraftKind, Identity};

#[derive(Debug, Clone, Copy)]
pub struct ReplyTarget<'a> {
    pub from: &'a Addr,
    pub to: &'a [Addr],
    pub cc: &'a [Addr],
    pub reply_to: &'a [Addr],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Addressing {
    pub from: Addr,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
}

#[must_use]
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
