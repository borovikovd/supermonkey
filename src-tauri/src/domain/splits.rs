use std::collections::HashMap;

use super::query::{MatchTarget, Query, RuleError};
use super::{
    Identity, LabelId, LabelSet, MessageFacts, PinKind, SplitId, SplitKind, SystemLabel, Thread,
};

pub const IMPORTANT_POSITION: i32 = 10_000;
pub const OTHER_POSITION: i32 = 20_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StarterSplit {
    pub name: &'static str,
    pub query: &'static str,
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split {
    pub id: SplitId,
    pub name: String,
    pub kind: SplitKind,
    pub position: i32,
    pub query: String,
    pub enabled: bool,
    pub hide_when_empty: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    pub kind: PinKind,
    pub value: String,
    pub split: SplitId,
}

impl Pin {
    #[must_use]
    pub fn new(kind: PinKind, value: &str, split: SplitId) -> Option<Self> {
        let normalized = value.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            return None;
        }
        Some(Self {
            kind,
            value: normalized,
            split,
        })
    }
}

#[must_use]
pub const fn split_position(kind: SplitKind, index: i32) -> i32 {
    match kind {
        SplitKind::Custom => index,
        SplitKind::Important => IMPORTANT_POSITION,
        SplitKind::Other => OTHER_POSITION,
    }
}

#[must_use]
pub fn email_domain(email: &str) -> Option<String> {
    let (_, domain) = email.trim().rsplit_once('@')?;
    let domain = domain.trim().to_ascii_lowercase();
    (!domain.is_empty()).then_some(domain)
}

#[must_use]
pub fn is_person_to_person(message: &MessageFacts) -> bool {
    if message.is_list {
        return false;
    }
    !message
        .labels
        .iter()
        .any(|l| SystemLabel::is_non_personal_category(l.as_str()))
}

pub fn check_split_rule(query: &str) -> Result<(), RuleError> {
    if query.trim().is_empty() {
        return Ok(());
    }
    Query::parse_rule(query).map(|_| ())
}

#[derive(Debug)]
struct CompiledSplit {
    id: SplitId,
    kind: SplitKind,
    enabled: bool,
    rule: Option<Query>,
}

#[derive(Debug)]
pub struct Router {
    splits: Vec<CompiledSplit>,
    order: HashMap<SplitId, usize>,
    sender_pins: HashMap<String, SplitId>,
    domain_pins: HashMap<String, SplitId>,
    label_pins: HashMap<String, SplitId>,
}

impl Router {
    #[must_use]
    pub fn new(mut splits: Vec<Split>, pins: Vec<Pin>) -> Self {
        splits.sort_by_key(|s| (s.position, s.id));
        let mut order = HashMap::with_capacity(splits.len());
        let compiled: Vec<CompiledSplit> = splits
            .into_iter()
            .enumerate()
            .map(|(idx, s)| {
                order.insert(s.id, idx);
                let rule = if s.kind == SplitKind::Custom && !s.query.trim().is_empty() {
                    Query::parse_rule(&s.query).ok().filter(|q| !q.is_empty())
                } else {
                    None
                };
                CompiledSplit {
                    id: s.id,
                    kind: s.kind,
                    enabled: s.enabled,
                    rule,
                }
            })
            .collect();

        let mut sender_pins = HashMap::new();
        let mut domain_pins = HashMap::new();
        let mut label_pins = HashMap::new();
        for pin in pins {
            match pin.kind {
                PinKind::Sender => {
                    sender_pins.insert(pin.value, pin.split);
                }
                PinKind::Domain => {
                    domain_pins.insert(pin.value, pin.split);
                }
                PinKind::Label => {
                    label_pins.insert(pin.value, pin.split);
                }
            }
        }
        Self {
            splits: compiled,
            order,
            sender_pins,
            domain_pins,
            label_pins,
        }
    }

    #[must_use]
    pub fn route(
        &self,
        thread: &Thread,
        identity: &Identity,
        user_labels: &HashMap<LabelId, String>,
    ) -> Option<SplitId> {
        let message = thread.latest_from_someone_else(identity)?;
        let labels = thread.labels();
        self.place_with_labels(message, &labels, user_labels)
    }

    #[must_use]
    pub fn place(
        &self,
        message: &MessageFacts,
        user_labels: &HashMap<LabelId, String>,
    ) -> Option<SplitId> {
        self.place_with_labels(message, &message.labels, user_labels)
    }

    fn place_with_labels(
        &self,
        message: &MessageFacts,
        labels: &LabelSet,
        user_labels: &HashMap<LabelId, String>,
    ) -> Option<SplitId> {
        if let Some(id) = self.pinned_split(message, labels, user_labels) {
            return Some(id);
        }
        let target = MatchTarget::rule_with_labels(message, labels, user_labels);
        for split in &self.splits {
            if !split.enabled || split.kind != SplitKind::Custom {
                continue;
            }
            if split.rule.as_ref().is_some_and(|q| q.matches(&target)) {
                return Some(split.id);
            }
        }
        if is_person_to_person(message)
            && let Some(imp) = self
                .splits
                .iter()
                .find(|s| s.kind == SplitKind::Important && s.enabled)
        {
            return Some(imp.id);
        }
        self.splits
            .iter()
            .find(|s| s.kind == SplitKind::Other && s.enabled)
            .map(|s| s.id)
    }

    fn is_enabled(&self, id: SplitId) -> bool {
        self.order
            .get(&id)
            .and_then(|&idx| self.splits.get(idx))
            .is_some_and(|s| s.enabled)
    }

    fn pinned_split(
        &self,
        message: &MessageFacts,
        labels: &LabelSet,
        user_labels: &HashMap<LabelId, String>,
    ) -> Option<SplitId> {
        let sender = message.from.key();
        if let Some(&split) = self.sender_pins.get(&sender)
            && self.is_enabled(split)
        {
            return Some(split);
        }
        if let Some(domain) = email_domain(&message.from.email)
            && let Some(&split) = self.domain_pins.get(&domain)
            && self.is_enabled(split)
        {
            return Some(split);
        }
        labels
            .iter()
            .filter_map(|label| {
                self.label_pins
                    .get(&label.as_str().to_ascii_lowercase())
                    .or_else(|| {
                        user_labels
                            .get(label)
                            .and_then(|name| self.label_pins.get(&name.trim().to_ascii_lowercase()))
                    })
                    .copied()
            })
            .filter(|&split| self.is_enabled(split))
            .min_by_key(|split| self.order.get(split).copied().unwrap_or(usize::MAX))
    }
}
