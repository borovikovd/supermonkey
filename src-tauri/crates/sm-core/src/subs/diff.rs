//! Turns the ids a list last sent and its ids now into a patch.
//!
//! The mirror applies a patch by removing `removed` and every moved key,
//! then inserting each move, in order, right after its `after` key
//! (`applyListPatch` in `src/store/mirror-core.ts`). Moves come in new-list
//! order and `after` is always the key just before in the new list, so it is
//! already in place when the move is applied.

use std::collections::{HashMap, HashSet};

use sm_ipc::{ListEntry, ListMove};

/// What a list patch carries besides its rows.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct ListDiff {
    /// Keys no longer listed.
    pub(crate) removed: Vec<String>,
    /// Keys inserted or moved, each after the key before it.
    pub(crate) moved: Vec<ListMove>,
    /// Keys in the new list whose rows changed: their rows go with the
    /// patch, inside the window or not.
    pub(crate) changed: Vec<String>,
}

impl ListDiff {
    /// Whether the patch would change nothing in the mirror.
    pub(crate) fn is_empty(&self) -> bool {
        self.removed.is_empty() && self.moved.is_empty() && self.changed.is_empty()
    }
}

/// The patch from `old` to `new`. `changed` says whether a key's row changed.
/// A key that stays in place but changes split counts as moved, so the patch
/// carries its new split.
pub(crate) fn diff(
    old: &[ListEntry],
    new: &[ListEntry],
    changed: impl Fn(&str) -> bool,
) -> ListDiff {
    let new_at: HashMap<&str, usize> = new
        .iter()
        .enumerate()
        .map(|(i, e)| (e.key.as_str(), i))
        .collect();
    let removed = old
        .iter()
        .filter(|e| !new_at.contains_key(e.key.as_str()))
        .map(|e| e.key.clone())
        .collect();
    // Kept keys in old order, as positions in the new list. The longest
    // increasing run of those positions can stay where it is, unless its
    // split changed.
    let kept: Vec<(usize, &ListEntry)> = old
        .iter()
        .filter_map(|e| new_at.get(e.key.as_str()).map(|&at| (at, e)))
        .collect();
    let stable: HashSet<&str> = longest_increasing(&kept)
        .into_iter()
        .filter(|(at, e)| new.get(*at).is_some_and(|now| now.split == e.split))
        .map(|(_, e)| e.key.as_str())
        .collect();
    let mut moved = Vec::new();
    let mut before: Option<&ListEntry> = None;
    for entry in new {
        if !stable.contains(entry.key.as_str()) {
            moved.push(ListMove {
                key: entry.key.clone(),
                split: entry.split,
                after: before.map(|b| b.key.clone()),
            });
        }
        before = Some(entry);
    }
    let changed = new
        .iter()
        .filter(|e| changed(&e.key))
        .map(|e| e.key.clone())
        .collect();
    ListDiff {
        removed,
        moved,
        changed,
    }
}

/// A longest strictly increasing run of `items` by position. O(n log n).
fn longest_increasing<'a>(items: &[(usize, &'a ListEntry)]) -> Vec<(usize, &'a ListEntry)> {
    // `tails[k]`: the index in `items` of the smallest tail of a run of k + 1.
    let mut tails: Vec<usize> = Vec::new();
    let mut prev: Vec<Option<usize>> = vec![None; items.len()];
    for (i, &(at, _)) in items.iter().enumerate() {
        let len = tails.partition_point(|&t| items.get(t).is_some_and(|&(p, _)| p < at));
        if let Some(slot) = prev.get_mut(i) {
            *slot = len.checked_sub(1).and_then(|k| tails.get(k).copied());
        }
        if let Some(tail) = tails.get_mut(len) {
            *tail = i;
        } else {
            tails.push(i);
        }
    }
    let mut run = Vec::with_capacity(tails.len());
    let mut cur = tails.last().copied();
    while let Some(i) = cur {
        if let Some(&item) = items.get(i) {
            run.push(item);
        }
        cur = prev.get(i).copied().flatten();
    }
    run.reverse();
    run
}

#[cfg(test)]
mod tests {
    use sm_domain::SplitId;

    use super::*;

    fn entries(keys: &str) -> Vec<ListEntry> {
        keys.split_whitespace()
            .map(|k| ListEntry {
                key: k.to_owned(),
                split: None,
            })
            .collect()
    }

    /// The mirror's `applyListPatch`, written out here rather than shared
    /// with the diff, so a bug in both can't cancel out.
    fn apply(list: &[ListEntry], d: &ListDiff) -> Vec<ListEntry> {
        let mut out: Vec<ListEntry> = list
            .iter()
            .filter(|e| !d.removed.contains(&e.key) && !d.moved.iter().any(|m| m.key == e.key))
            .cloned()
            .collect();
        for m in &d.moved {
            let at = m.after.as_ref().map_or(0, |after| {
                out.iter()
                    .position(|e| &e.key == after)
                    .map_or(out.len(), |p| p + 1)
            });
            out.insert(
                at,
                ListEntry {
                    key: m.key.clone(),
                    split: m.split,
                },
            );
        }
        out
    }

    fn keys(d: &ListDiff) -> (Vec<&str>, Vec<(&str, Option<&str>)>) {
        (
            d.removed.iter().map(String::as_str).collect(),
            d.moved
                .iter()
                .map(|m| (m.key.as_str(), m.after.as_deref()))
                .collect(),
        )
    }

    #[test]
    fn the_same_list_is_an_empty_patch() {
        let list = entries("a b c");
        let d = diff(&list, &list, |_| false);
        assert!(d.is_empty(), "{d:?}");
    }

    #[test]
    fn a_removal_is_listed_and_nothing_moves() {
        let d = diff(&entries("a b c"), &entries("a c"), |_| false);
        assert_eq!(keys(&d), (vec!["b"], vec![]));
    }

    #[test]
    fn a_thread_bumped_to_the_top_is_one_move_to_the_front() {
        let old = entries("a b c d");
        let new = entries("d a b c");
        let d = diff(&old, &new, |k| k == "d");
        assert_eq!(keys(&d), (vec![], vec![("d", None)]));
        assert_eq!(d.changed, ["d"]);
        assert_eq!(apply(&old, &d), new);
    }

    #[test]
    fn an_insert_follows_the_key_before_it() {
        let old = entries("a c");
        let new = entries("a b c");
        let d = diff(&old, &new, |_| false);
        assert_eq!(keys(&d), (vec![], vec![("b", Some("a"))]));
        assert_eq!(apply(&old, &d), new);
    }

    #[test]
    fn a_split_change_in_place_is_a_move_with_the_new_split() {
        let old = entries("a b");
        let mut new = old.clone();
        new[1].split = Some(SplitId(2));
        let d = diff(&old, &new, |_| false);
        assert_eq!(keys(&d), (vec![], vec![("b", Some("a"))]));
        assert_eq!(d.moved[0].split, Some(SplitId(2)));
        assert_eq!(apply(&old, &d), new);
    }

    #[test]
    fn changed_rows_are_only_those_still_listed() {
        let d = diff(&entries("a b c"), &entries("a c"), |k| k != "a");
        assert_eq!(d.changed, ["c"], "b left the list; a did not change");
    }

    #[test]
    fn a_reversed_list_rebuilds_with_few_moves() {
        let old = entries("a b c d e");
        let new = entries("e d c b a");
        let d = diff(&old, &new, |_| false);
        assert_eq!(apply(&old, &d), new);
        assert_eq!(d.moved.len(), 4, "one key stays put");
    }

    #[test]
    fn any_reordering_rebuilds_the_new_list() {
        // Every permutation of four keys, with one dropped and one added.
        let old = entries("a b c d");
        let perms = [
            "d c b a",
            "b a d c",
            "c a d",
            "x a b c d",
            "a b c d x",
            "d x a",
            "x",
            "",
        ];
        for p in perms {
            let new = entries(p);
            let d = diff(&old, &new, |_| false);
            assert_eq!(apply(&old, &d), new, "to {p:?}");
        }
    }
}
