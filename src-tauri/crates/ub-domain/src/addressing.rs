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
/// [`reply_addressing`]. A reply with no target starts like new mail.
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
pub fn reply_addressing(target: ReplyTarget<'_>, me: &Identity, reply_all: bool) -> Addressing {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SendAsAddress;

    fn me() -> Identity {
        Identity::new(
            "me@x.io".into(),
            "Me".into(),
            vec![
                SendAsAddress {
                    email: "me@x.io".into(),
                    name: "Me".into(),
                    is_default: true,
                    is_primary: true,
                },
                SendAsAddress {
                    email: "work@corp.io".into(),
                    name: "Me at Work".into(),
                    is_default: false,
                    is_primary: false,
                },
            ],
        )
    }

    fn target<'a>(
        from: &'a Addr,
        to: &'a [Addr],
        cc: &'a [Addr],
        reply_to: &'a [Addr],
    ) -> ReplyTarget<'a> {
        ReplyTarget {
            from,
            to,
            cc,
            reply_to,
        }
    }

    #[test]
    fn reply_goes_to_the_sender_from_the_address_it_was_sent_to() {
        let alice = Addr::named("Alice", "alice@a.io");
        let to = [Addr::new("WORK@corp.io"), Addr::new("bob@b.io")];
        let got = reply_addressing(target(&alice, &to, &[], &[]), &me(), false);
        assert_eq!(got.from, Addr::named("Me at Work", "work@corp.io"));
        assert_eq!(got.to, [alice]);
        assert_eq!(got.cc, Vec::<Addr>::new());
    }

    #[test]
    fn reply_uses_reply_to_and_the_default_when_no_send_as_matches() {
        let alice = Addr::new("alice@a.io");
        let reply_to = [Addr::new("list@a.io")];
        let to = [Addr::new("someone@else.io")];
        let got = reply_addressing(target(&alice, &to, &[], &reply_to), &me(), false);
        assert_eq!(got.from, Addr::named("Me", "me@x.io"));
        assert_eq!(got.to, reply_to);
    }

    #[test]
    fn to_beats_cc_when_both_hold_a_send_as_address() {
        let alice = Addr::new("alice@a.io");
        let to = [Addr::new("me@x.io")];
        let cc = [Addr::new("work@corp.io")];
        let got = reply_addressing(target(&alice, &to, &cc, &[]), &me(), false);
        assert_eq!(got.from.email, "me@x.io");
    }

    #[test]
    fn reply_all_ccs_the_rest_without_me_or_duplicates() {
        let alice = Addr::new("alice@a.io");
        let to = [
            Addr::new("me@x.io"),
            Addr::new("bob@b.io"),
            Addr::new("ALICE@a.io"),
        ];
        let cc = [
            Addr::new("carol@c.io"),
            Addr::new("Bob@b.io"),
            Addr::new("work@corp.io"),
        ];
        let got = reply_addressing(target(&alice, &to, &cc, &[]), &me(), true);
        assert_eq!(got.to, [alice]);
        assert_eq!(got.cc, [Addr::new("bob@b.io"), Addr::new("carol@c.io")]);
    }

    #[test]
    fn replying_to_my_own_message_answers_its_recipients() {
        let mine = Addr::named("Me at Work", "work@corp.io");
        let to = [Addr::new("bob@b.io")];
        let cc = [Addr::new("me@x.io"), Addr::new("carol@c.io")];
        let got = reply_addressing(target(&mine, &to, &cc, &[]), &me(), true);
        assert_eq!(got.from, mine);
        assert_eq!(got.to, to);
        assert_eq!(got.cc, [Addr::new("carol@c.io")]);
        let single = reply_addressing(target(&mine, &to, &cc, &[]), &me(), false);
        assert_eq!(single.cc, Vec::<Addr>::new());
    }

    #[test]
    fn new_and_forward_start_empty_from_the_default() {
        let alice = Addr::new("alice@a.io");
        let to = [Addr::new("work@corp.io")];
        for kind in [DraftKind::New, DraftKind::Forward] {
            let got = addressing(kind, Some(target(&alice, &to, &[], &[])), &me(), true);
            assert_eq!(got.from, Addr::named("Me", "me@x.io"), "{kind:?}");
            assert!(got.to.is_empty() && got.cc.is_empty(), "{kind:?}");
        }
        let reply = addressing(
            DraftKind::Reply,
            Some(target(&alice, &to, &[], &[])),
            &me(),
            false,
        );
        assert_eq!(reply.to, [alice]);
    }
}
