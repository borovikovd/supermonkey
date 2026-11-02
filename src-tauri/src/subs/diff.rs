//! Turns an old and a new ordered id list into removals and moves.
//!
//! The mirror applies a patch by removing `removed` and every moved key, then
//! inserting each move, in order, right after its `after` key. Moves come in
//! new-list order and `after` is always the key just before in the new list,
//! so it is already in place when the move is applied.

use std::collections::{HashMap, HashSet};

use crate::model::{ListEntry, ListMove};

pub fn diff(old: &[ListEntry], new: &[ListEntry]) -> (Vec<String>, Vec<ListMove>) {
    let new_pos: HashMap<&str, usize> = new
        .iter()
        .enumerate()
        .map(|(i, e)| (e.key.as_str(), i))
        .collect();
    let old_split: HashMap<&str, Option<i64>> =
        old.iter().map(|e| (e.key.as_str(), e.split)).collect();

    let removed: Vec<String> = old
        .iter()
        .filter(|e| !new_pos.contains_key(e.key.as_str()))
        .map(|e| e.key.clone())
        .collect();

    // Kept keys in old order, as positions in the new list. The longest
    // increasing run of those positions can stay where it is.
    let kept: Vec<(usize, &str)> = old
        .iter()
        .filter_map(|e| new_pos.get(e.key.as_str()).map(|&p| (p, e.key.as_str())))
        .collect();
    let stable: HashSet<&str> = longest_increasing(&kept)
        .into_iter()
        .filter(|k| {
            let i = new_pos[k];
            old_split.get(k).copied().flatten() == new[i].split
        })
        .collect();

    let mut moved = Vec::new();
    for (i, e) in new.iter().enumerate() {
        if !stable.contains(e.key.as_str()) {
            moved.push(ListMove {
                key: e.key.clone(),
                split: e.split,
                after: (i > 0).then(|| new[i - 1].key.clone()),
            });
        }
    }
    (removed, moved)
}

/// Keys of a longest strictly increasing subsequence by position. O(n log n).
fn longest_increasing<'a>(items: &[(usize, &'a str)]) -> Vec<&'a str> {
    let mut tails: Vec<usize> = Vec::new(); // index into items
    let mut prev: Vec<Option<usize>> = vec![None; items.len()];
    for (i, &(pos, _)) in items.iter().enumerate() {
        let at = tails.partition_point(|&t| items[t].0 < pos);
        if at > 0 {
            prev[i] = Some(tails[at - 1]);
        }
        if at == tails.len() {
            tails.push(i);
        } else {
            tails[at] = i;
        }
    }
    let mut out = Vec::with_capacity(tails.len());
    let mut cur = tails.last().copied();
    while let Some(i) = cur {
        out.push(items[i].1);
        cur = prev[i];
    }
    out.reverse();
    out
}

/// Applies a patch the way the mirror does. Used by tests.
pub fn apply(list: &[ListEntry], removed: &[String], moved: &[ListMove]) -> Vec<ListEntry> {
    let drop: HashSet<&str> = removed
        .iter()
        .map(String::as_str)
        .chain(moved.iter().map(|m| m.key.as_str()))
        .collect();
    let mut out: Vec<ListEntry> = list
        .iter()
        .filter(|e| !drop.contains(e.key.as_str()))
        .cloned()
        .collect();
    for m in moved {
        let at = match &m.after {
            None => 0,
            Some(a) => out
                .iter()
                .position(|e| &e.key == a)
                .map_or(out.len(), |p| p + 1),
        };
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

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn entries(keys: &[u32], split_of: impl Fn(u32) -> Option<i64>) -> Vec<ListEntry> {
        keys.iter()
            .map(|k| ListEntry {
                key: k.to_string(),
                split: split_of(*k),
            })
            .collect()
    }

    #[test]
    fn moves_only_what_moved() {
        let old = entries(&[1, 2, 3, 4, 5], |_| None);
        let new = entries(&[1, 3, 4, 2, 5], |_| None);
        let (removed, moved) = diff(&old, &new);
        assert_eq!(removed, Vec::<String>::new());
        assert_eq!(moved.len(), 1);
        assert_eq!(moved[0].key, "2");
        assert_eq!(apply(&old, &removed, &moved), new);
    }

    #[test]
    fn split_change_is_a_move() {
        let old = entries(&[1, 2], |_| Some(1));
        let new = vec![
            ListEntry {
                key: "1".into(),
                split: Some(1),
            },
            ListEntry {
                key: "2".into(),
                split: Some(2),
            },
        ];
        let (_, moved) = diff(&old, &new);
        assert_eq!(moved.len(), 1);
        assert_eq!(apply(&old, &[], &moved), new);
    }

    proptest! {
        #[test]
        fn patch_rebuilds_new_list(
            old in proptest::sample::subsequence((0u32..60).collect::<Vec<_>>(), 0..60).prop_shuffle(),
            new in proptest::sample::subsequence((0u32..60).collect::<Vec<_>>(), 0..60).prop_shuffle(),
            salt in 0u32..3,
        ) {
            let old = entries(&old, |k| Some(i64::from((k + salt) % 3)));
            let new = entries(&new, |k| Some(i64::from(k % 3)));
            let (removed, moved) = diff(&old, &new);
            prop_assert_eq!(apply(&old, &removed, &moved), new);
        }
    }
}
