use std::collections::HashMap;

use crate::{
    Identity, LabelSet, MessageFacts, PinKind, SplitId, SplitKind, SystemLabel, Thread,
    ThreadDigest,
    labels::is_category_id,
    query::{MatchTarget, Query, RuleError},
};

pub const IMPORTANT_POSITION: i32 = 900_000;
pub const OTHER_POSITION: i32 = 1_000_000;

pub fn check_split_rule(query: &str) -> Result<(), RuleError> {
    if query.trim().is_empty() {
        return Ok(());
    }
    Query::parse_rule(query).map(drop)
}

#[must_use]
pub fn split_position(kind: SplitKind, custom_rank: u32) -> i32 {
    match kind {
        SplitKind::Custom => i32::try_from(custom_rank.saturating_add(1).saturating_mul(10))
            .unwrap_or(IMPORTANT_POSITION - 1)
            .min(IMPORTANT_POSITION - 1),
        SplitKind::Important => IMPORTANT_POSITION,
        SplitKind::Other => OTHER_POSITION,
    }
}

const NON_PERSONAL: [SystemLabel; 4] = [
    SystemLabel::CategoryPromotions,
    SystemLabel::CategorySocial,
    SystemLabel::CategoryUpdates,
    SystemLabel::CategoryForums,
];

pub(crate) fn is_person_to_person(labels: &LabelSet, is_list: bool) -> bool {
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
    pub kind: SplitKind,
    pub query: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Pin {
    pub kind: PinKind,
    value: String,
    pub split: SplitId,
}

impl Pin {
    #[must_use]
    pub fn new(kind: PinKind, value: &str, split: SplitId) -> Option<Self> {
        let value = value.trim().to_lowercase();
        (!value.is_empty()).then_some(Self { kind, value, split })
    }

    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }

    #[must_use]
    pub fn into_value(self) -> String {
        self.value
    }
}

#[must_use]
pub fn email_domain(email: &str) -> Option<String> {
    email
        .rsplit_once('@')
        .map(|(_, domain)| domain.to_lowercase())
        .filter(|d| !d.is_empty())
}

#[derive(Debug, Clone)]
pub struct Router {
    splits: Vec<RouteSplit>,
    pins: HashMap<PinKind, HashMap<String, SplitId>>,
}

#[derive(Debug, Clone)]
struct RouteSplit {
    id: SplitId,
    kind: SplitKind,
    enabled: bool,
    rule: Option<Query>,
}

impl Router {
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
        let mut by_kind: HashMap<PinKind, HashMap<String, SplitId>> = HashMap::new();
        for p in pins {
            by_kind.entry(p.kind).or_default().insert(p.value, p.split);
        }
        Self {
            splits,
            pins: by_kind,
        }
    }

    #[must_use]
    pub fn route(&self, message: &MessageFacts, thread_label_names: &[&str]) -> Option<SplitId> {
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

    #[must_use]
    pub fn place(
        &self,
        thread: &Thread,
        digest: &ThreadDigest<'_>,
        me: &Identity,
        thread_label_names: &[&str],
    ) -> Option<SplitId> {
        if !digest.in_inbox() {
            return None;
        }
        let message = thread.latest_from_someone_else(me)?;
        self.route(message, thread_label_names)
    }

    fn pinned(&self, message: &MessageFacts, thread_label_names: &[&str]) -> Option<SplitId> {
        let pin = |kind: PinKind, value: &str| {
            self.pins
                .get(&kind)
                .and_then(|values| values.get(value))
                .copied()
                .filter(|&id| self.is_enabled(id))
        };
        let email = message.from.email.to_lowercase();
        pin(PinKind::Sender, &email)
            .or_else(|| email_domain(&email).and_then(|d| pin(PinKind::Domain, &d)))
            .or_else(|| {
                thread_label_names
                    .iter()
                    .filter_map(|name| pin(PinKind::Label, &name.to_lowercase()))
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
