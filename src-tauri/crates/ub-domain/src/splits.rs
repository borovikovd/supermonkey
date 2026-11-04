//! Split Inbox (SPEC Split Inbox): where an Inbox thread lands, the
//! person-to-person rule, pins, and the starter splits.

use std::collections::HashMap;

use crate::{
    Identity, LabelSet, MessageFacts, PinKind, SplitId, SplitKind, SystemLabel, Thread,
    ThreadDigest,
    labels::is_category_id,
    query::{MatchTarget, Query},
};

/// Where Important sits in split order: after every custom split.
pub const IMPORTANT_POSITION: i32 = 900_000;
/// Where Other sits: last.
pub const OTHER_POSITION: i32 = 1_000_000;

/// The position the settings sheet's save gives a split: custom splits by
/// their place among custom splits (`custom_rank`, from 0), then Important,
/// then Other.
pub fn split_position(kind: SplitKind, custom_rank: u32) -> i32 {
    match kind {
        SplitKind::Custom => i32::try_from(custom_rank.saturating_add(1).saturating_mul(10))
            .unwrap_or(IMPORTANT_POSITION - 1)
            .min(IMPORTANT_POSITION - 1),
        SplitKind::Important => IMPORTANT_POSITION,
        SplitKind::Other => OTHER_POSITION,
    }
}

/// Category labels that keep a message out of Important.
const NON_PERSONAL: [SystemLabel; 4] = [
    SystemLabel::CategoryPromotions,
    SystemLabel::CategorySocial,
    SystemLabel::CategoryUpdates,
    SystemLabel::CategoryForums,
];

/// Whether a message looks person-to-person, which puts its thread in
/// Important: not list mail, none of `CATEGORY_PROMOTIONS`, `_SOCIAL`,
/// `_UPDATES`, `_FORUMS`, and either `CATEGORY_PERSONAL` or no category at
/// all. Ids compare ignoring ASCII case.
pub fn is_person_to_person(labels: &LabelSet, is_list: bool) -> bool {
    if is_list {
        return false;
    }
    let has = |label: SystemLabel| {
        labels
            .iter()
            .any(|l| l.as_str().eq_ignore_ascii_case(label.id()))
    };
    if NON_PERSONAL.into_iter().any(has) {
        return false;
    }
    has(SystemLabel::CategoryPersonal) || !labels.iter().any(|l| is_category_id(l.as_str()))
}

/// A custom split the settings sheet offers, off until turned on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StarterSplit {
    /// Its name.
    pub name: &'static str,
    /// Its rule.
    pub query: &'static str,
}

/// The four starter splits, in the order the sheet lists them.
pub const STARTER_SPLITS: [StarterSplit; 4] = [
    StarterSplit {
        name: "Newsletters",
        query: "is:list",
    },
    StarterSplit {
        name: "Promotions",
        query: "label:CATEGORY_PROMOTIONS",
    },
    StarterSplit {
        name: "Social",
        query: "label:CATEGORY_SOCIAL",
    },
    StarterSplit {
        name: "Updates",
        query: "label:CATEGORY_UPDATES",
    },
];

/// A split as routing reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split {
    /// Its id.
    pub id: SplitId,
    /// Custom, Important, or Other.
    pub kind: SplitKind,
    /// A custom split's rule; empty for the built-in ones.
    pub query: String,
    /// Turned on. Other always is.
    pub enabled: bool,
}

/// A pin: mail from a sender, a domain, or with a user label goes to a split.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Pin {
    /// What it matches.
    pub kind: PinKind,
    value: String,
    /// The split it sends mail to.
    pub split: SplitId,
}

impl Pin {
    /// A pin, with its value trimmed and lowercased as pins are stored and
    /// matched. `None` when the value is blank.
    pub fn new(kind: PinKind, value: &str, split: SplitId) -> Option<Self> {
        let value = value.trim().to_lowercase();
        (!value.is_empty()).then_some(Self { kind, value, split })
    }

    /// The sender address, domain, or label name, lowercased.
    pub fn value(&self) -> &str {
        &self.value
    }
}

/// The domain of an address: what follows its last `@`, lowercased.
pub fn email_domain(email: &str) -> Option<String> {
    email
        .rsplit_once('@')
        .map(|(_, domain)| domain.to_lowercase())
        .filter(|d| !d.is_empty())
}

/// One account's splits and pins, ready to route threads. Build it once per
/// change to them: it parses every custom rule.
#[derive(Debug, Clone)]
pub struct Router {
    /// In split order.
    splits: Vec<RouteSplit>,
    pins: HashMap<(PinKind, String), SplitId>,
}

#[derive(Debug, Clone)]
struct RouteSplit {
    id: SplitId,
    kind: SplitKind,
    enabled: bool,
    /// A custom split's parsed rule; `None` for a blank rule, which matches nothing.
    rule: Option<Query>,
}

impl Router {
    /// A router over `splits`, given in split order, and `pins`. Stored rules
    /// were checked strictly when saved, so they parse leniently here.
    pub fn new(
        splits: impl IntoIterator<Item = Split>,
        pins: impl IntoIterator<Item = Pin>,
    ) -> Self {
        let splits = splits
            .into_iter()
            .map(|s| RouteSplit {
                id: s.id,
                kind: s.kind,
                enabled: s.enabled,
                rule: (s.kind == SplitKind::Custom && !s.query.trim().is_empty())
                    .then(|| Query::parse(&s.query)),
            })
            .collect();
        let pins = pins
            .into_iter()
            .map(|p| ((p.kind, p.value), p.split))
            .collect();
        Self { splits, pins }
    }

    /// The split a message lands in (SPEC Where a conversation lands), given
    /// the names of its thread's user labels. First match wins:
    /// 1. a pin whose split is enabled: sender, then domain, then label (the
    ///    first in split order when several label pins match);
    /// 2. enabled custom splits, top to bottom;
    /// 3. Important, when enabled and the message is person-to-person;
    /// 4. Other.
    pub fn route(&self, message: &MessageFacts, thread_label_names: &[String]) -> Option<SplitId> {
        self.pinned(message, thread_label_names)
            .or_else(|| {
                let target = MatchTarget::rule(message, thread_label_names);
                self.splits
                    .iter()
                    .filter(|s| s.enabled)
                    .find(|s| s.rule.as_ref().is_some_and(|q| q.matches(&target)))
                    .map(|s| s.id)
            })
            .or_else(|| {
                self.splits
                    .iter()
                    .find(|s| s.kind == SplitKind::Important && s.enabled)
                    .filter(|_| is_person_to_person(&message.labels, message.is_list))
                    .map(|s| s.id)
            })
            .or_else(|| {
                self.splits
                    .iter()
                    .find(|s| s.kind == SplitKind::Other)
                    .map(|s| s.id)
            })
    }

    /// The split a thread lands in: `None` unless it is in the Inbox. Routes
    /// its latest message from someone else.
    pub fn place(
        &self,
        thread: &Thread,
        digest: &ThreadDigest,
        me: &Identity,
        thread_label_names: &[String],
    ) -> Option<SplitId> {
        if !digest.in_inbox() {
            return None;
        }
        let message = thread.latest_from_someone_else(me)?;
        self.route(message, thread_label_names)
    }

    fn pinned(&self, message: &MessageFacts, thread_label_names: &[String]) -> Option<SplitId> {
        let pin = |kind: PinKind, value: String| {
            self.pins
                .get(&(kind, value))
                .copied()
                .filter(|&id| self.is_enabled(id))
        };
        let email = message.from.email.to_lowercase();
        pin(PinKind::Sender, email.clone())
            .or_else(|| email_domain(&email).and_then(|d| pin(PinKind::Domain, d)))
            .or_else(|| {
                thread_label_names
                    .iter()
                    .filter_map(|name| pin(PinKind::Label, name.to_lowercase()))
                    .min_by_key(|&id| self.order(id))
            })
    }

    fn is_enabled(&self, id: SplitId) -> bool {
        self.splits.iter().any(|s| s.id == id && s.enabled)
    }

    fn order(&self, id: SplitId) -> usize {
        self.splits
            .iter()
            .position(|s| s.id == id)
            .unwrap_or(usize::MAX)
    }
}
