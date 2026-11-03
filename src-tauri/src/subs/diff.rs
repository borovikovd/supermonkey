//! Turns an old and a new ordered id list into removals and moves.
//!
//! The mirror applies a patch by removing `removed` and every moved key, then
//! inserting each move, in order, right after its `after` key. Moves come in
//! new-list order and `after` is always the key just before in the new list,
//! so it is already in place when the move is applied.

use std::collections::{HashMap, HashSet};

use crate::model::{ListEntry, ListMove, SplitId};

pub fn diff(old: &[ListEntry], new: &[ListEntry]) -> (Vec<String>, Vec<ListMove>) {
    let new_pos: HashMap<&str, usize> = new
        .iter()
        .enumerate()
        .map(|(i, e)| (e.key.as_str(), i))
        .collect();
    let old_split: HashMap<&str, Option<SplitId>> =
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
