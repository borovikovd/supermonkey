//! The Important rule and the starter custom splits.

/// Category labels that keep a message out of Important.
const NON_PERSONAL_CATEGORIES: &[&str] = &[
    "CATEGORY_PROMOTIONS",
    "CATEGORY_SOCIAL",
    "CATEGORY_UPDATES",
    "CATEGORY_FORUMS",
];

/// Whether a message looks person-to-person, which puts its thread in
/// Important: it is not list mail, carries none of `CATEGORY_PROMOTIONS`,
/// `CATEGORY_SOCIAL`, `CATEGORY_UPDATES`, `CATEGORY_FORUMS`, and has either
/// `CATEGORY_PERSONAL` or no `CATEGORY_*` label at all. Ids compare
/// case-insensitively.
pub fn is_person_to_person(message_label_ids: &[&str], is_list: bool) -> bool {
    if is_list {
        return false;
    }
    let has = |id: &str| message_label_ids.iter().any(|l| l.eq_ignore_ascii_case(id));
    if NON_PERSONAL_CATEGORIES.iter().any(|c| has(c)) {
        return false;
    }
    let any_category = message_label_ids.iter().any(|l| {
        l.get(..9)
            .is_some_and(|p| p.eq_ignore_ascii_case("CATEGORY_"))
    });
    has("CATEGORY_PERSONAL") || !any_category
}

/// A custom split the settings sheet offers, off until turned on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StarterSplit {
    pub name: &'static str,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::{Address, MessageFields, Mode, parse_strict};

    #[test]
    fn person_to_person() {
        assert!(is_person_to_person(&[], false));
        assert!(is_person_to_person(&["INBOX", "UNREAD"], false));
        assert!(is_person_to_person(&["INBOX", "CATEGORY_PERSONAL"], false));
        assert!(is_person_to_person(&["category_personal"], false));
        assert!(!is_person_to_person(&["INBOX"], true));
        assert!(!is_person_to_person(&["CATEGORY_PERSONAL"], true));
        for c in NON_PERSONAL_CATEGORIES {
            assert!(!is_person_to_person(&["INBOX", c], false), "{c}");
            assert!(
                !is_person_to_person(&["CATEGORY_PERSONAL", c], false),
                "{c}"
            );
        }
        // An unknown category without CATEGORY_PERSONAL is not "no category".
        assert!(!is_person_to_person(&["CATEGORY_PURCHASES"], false));
        // User label ids are not categories.
        assert!(is_person_to_person(&["Label_12"], false));
    }

    #[test]
    fn starters_parse_and_match() {
        let names: Vec<_> = STARTER_SPLITS.iter().map(|s| s.name).collect();
        assert_eq!(names, ["Newsletters", "Promotions", "Social", "Updates"]);

        let mut m = MessageFields {
            from: Address::new("Shop", "deals@shop.com"),
            message_label_ids: vec!["INBOX".into(), "CATEGORY_PROMOTIONS".into()],
            is_list: true,
            ..Default::default()
        };
        let hits = |m: &MessageFields| -> Vec<&str> {
            STARTER_SPLITS
                .iter()
                .filter(|s| parse_strict(s.query).unwrap().matches(m, Mode::Rule))
                .map(|s| s.name)
                .collect()
        };
        assert_eq!(hits(&m), ["Newsletters", "Promotions"]);
        m.is_list = false;
        m.message_label_ids = vec!["CATEGORY_SOCIAL".into()];
        assert_eq!(hits(&m), ["Social"]);
    }
}
