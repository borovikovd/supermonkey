//! Mail actions: archive, return to inbox, snooze, trash, spam, star, read,
//! labels (SPEC Archive, trash, star, read; Labels; Snooze; Undo).
//!
//! An action carries its resolved effect. Rust applies it to the thread's
//! non-draft messages as they are now, records exactly what changed for undo,
//! and queues one Gmail `batchModify` with those message ids.

use std::collections::BTreeSet;

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::model::{
    AccountId, MessageId, Millis, ModifyAction, ModifyKind, SnoozeChange, ThreadId,
};
use crate::threads;
use crate::writer::Ctx;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MsgChange {
    pub msg: MessageId,
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadChange {
    pub thread: ThreadId,
    pub changes: Vec<MsgChange>,
    pub snooze_before: Option<Millis>,
    pub snooze_after: Option<Millis>,
}

/// Gmail op payload for `messages.batchModify`. Label ids are local ids.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ModifyOp {
    pub messages: Vec<String>,
    pub add: Vec<String>,
    pub remove: Vec<String>,
    /// For rollback on rejection: what each message changed.
    pub threads: Vec<ThreadChange>,
}

/// What one message's labels become under an action, or None when nothing
/// changes. Mark unread touches the latest message only, as Gmail does.
fn label_change(
    msg: &threads::Msg,
    add: &BTreeSet<String>,
    remove: &BTreeSet<String>,
    latest_live: Option<MessageId>,
) -> Option<(Vec<String>, MsgChange)> {
    let added: Vec<String> = add
        .iter()
        .filter(|l| {
            !(l.as_str() == "UNREAD" && latest_live.is_some() && Some(msg.id) != latest_live)
        })
        .filter(|l| !msg.has(l))
        .cloned()
        .collect();
    let removed: Vec<String> = remove.iter().filter(|l| msg.has(l)).cloned().collect();
    if added.is_empty() && removed.is_empty() {
        return None;
    }
    let mut labels: Vec<String> = msg
        .labels
        .iter()
        .filter(|l| !removed.contains(l))
        .cloned()
        .collect();
    labels.extend(added.iter().cloned());
    Some((
        labels,
        MsgChange {
            msg: msg.id,
            added,
            removed,
        },
    ))
}

/// The thread's snooze time after an action. Archive, trash, and spam also clear a snooze.
fn snooze_after(m: &ModifyAction, before: Option<i64>) -> Option<i64> {
    match &m.snooze {
        Some(SnoozeChange::Set(at)) => Some(*at),
        Some(SnoozeChange::Clear) => None,
        None if matches!(
            m.kind,
            ModifyKind::Done | ModifyKind::Trash | ModifyKind::Spam
        ) =>
        {
            None
        }
        None => before,
    }
}

pub fn modify(ctx: &mut Ctx<'_>, n: i64, m: &ModifyAction) -> anyhow::Result<()> {
    let add: BTreeSet<String> = m.add.iter().cloned().collect();
    let remove: BTreeSet<String> = m
        .remove
        .iter()
        .filter(|&l| !add.contains(l))
        .cloned()
        .collect();
    // Mark unread changes the latest message only, so it can't share an op
    // with labels meant for every message.
    let unread = add.contains("UNREAD");
    anyhow::ensure!(
        !unread || (add.len() == 1 && remove.is_empty()),
        "mark unread can't change other labels"
    );
    let mut changes: Vec<ThreadChange> = Vec::new();
    let mut gmail_ids: Vec<String> = Vec::new();

    for &thread in &m.threads {
        let Some((account, snooze_before)) = ctx
            .tx
            .query_row(
                "SELECT account_id, snooze_until FROM threads WHERE id = ?1",
                [thread],
                |r| Ok((r.get::<_, AccountId>(0)?, r.get::<_, Option<Millis>>(1)?)),
            )
            .optional()?
        else {
            continue;
        };
        anyhow::ensure!(
            account == m.account,
            "thread {thread} is not in account {}",
            m.account
        );
        let msgs = threads::messages(ctx.tx, thread)?;
        let latest_live = msgs.iter().rev().find(|x| x.live()).map(|x| x.id);
        let mut tc = ThreadChange {
            thread,
            changes: Vec::new(),
            snooze_before,
            snooze_after: snooze_after(m, snooze_before),
        };
        for msg in msgs.iter().filter(|x| !x.is_draft) {
            // Every message is part of the op (a queued action applies to the
            // messages it recorded), except that mark unread sends the latest only.
            if !unread || latest_live.is_none_or(|latest| latest == msg.id) {
                gmail_ids.push(msg.gmail_id.clone());
            }
            if let Some((labels, change)) = label_change(msg, &add, &remove, latest_live) {
                threads::set_message_labels(ctx, msg.id, &labels)?;
                tc.changes.push(change);
            }
        }
        if tc.snooze_after != snooze_before {
            ctx.tx.execute(
                "UPDATE threads SET snooze_until = ?2 WHERE id = ?1",
                params![thread, tc.snooze_after],
            )?;
            ctx.effect(crate::writer::Effect::Snoozes);
        }
        threads::refresh(ctx, thread)?;
        changes.push(tc);
    }

    if m.undoable && !changes.is_empty() {
        crate::undo::record(
            ctx,
            n,
            m.account,
            "mail",
            m.kind.label(),
            &serde_json::json!({ "threads": changes }),
        )?;
    }
    let any_label_change = changes.iter().any(|c| !c.changes.is_empty());
    if any_label_change && !gmail_ids.is_empty() {
        crate::queue::push(
            ctx,
            m.account,
            Some(n),
            "modify",
            &ModifyOp {
                messages: gmail_ids,
                add: add.into_iter().collect(),
                remove: remove.into_iter().collect(),
                threads: changes,
            },
        )?;
    }
    Ok(())
}

/// Reverts a mail entry: only the labels and snooze values it changed.
pub fn undo(
    ctx: &mut Ctx<'_>,
    n: i64,
    account: AccountId,
    payload: &serde_json::Value,
) -> anyhow::Result<()> {
    let threads = Vec::<ThreadChange>::deserialize(&payload["threads"])?;
    revert(ctx, Some(n), account, &threads, true)
}

/// Shared by undo and by rollback after a Gmail rejection.
pub fn revert(
    ctx: &mut Ctx<'_>,
    action: Option<i64>,
    account: AccountId,
    changes: &[ThreadChange],
    queue: bool,
) -> anyhow::Result<()> {
    // Group Gmail calls by identical label sets.
    let mut ops: Vec<ModifyOp> = Vec::new();
    for tc in changes {
        let Some(snooze_now) = ctx
            .tx
            .query_row(
                "SELECT snooze_until FROM threads WHERE id = ?1",
                [tc.thread],
                |r| r.get::<_, Option<Millis>>(0),
            )
            .optional()?
        else {
            continue;
        };
        let restores_snooze = snooze_now == tc.snooze_after && tc.snooze_after != tc.snooze_before;
        for c in &tc.changes {
            let Some((gmail_id, labels)) = ctx
                .tx
                .query_row(
                    "SELECT gmail_id, label_ids FROM messages WHERE id = ?1",
                    [c.msg],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                )
                .optional()?
            else {
                continue;
            };
            let labels: Vec<String> = crate::db::from_json(&labels);
            // Remove what the action added, add back what it removed.
            let remove: Vec<String> = c
                .added
                .iter()
                .filter(|l| labels.contains(l))
                .cloned()
                .collect();
            let add: Vec<String> = c
                .removed
                .iter()
                .filter(|l| !labels.contains(l))
                .cloned()
                .collect();
            if add.is_empty() && remove.is_empty() {
                continue;
            }
            let mut next: Vec<String> =
                labels.into_iter().filter(|l| !remove.contains(l)).collect();
            next.extend(add.iter().cloned());
            threads::set_message_labels(ctx, c.msg, &next)?;
            let i = ops
                .iter()
                .position(|o| o.add == add && o.remove == remove)
                .unwrap_or_else(|| {
                    ops.push(ModifyOp {
                        add: add.clone(),
                        remove: remove.clone(),
                        ..ModifyOp::default()
                    });
                    ops.len() - 1
                });
            let Some(op) = ops.get_mut(i) else { continue };
            op.messages.push(gmail_id);
            // What this undo changed, so a rejection by Gmail can roll it back.
            let change = MsgChange {
                msg: c.msg,
                added: add,
                removed: remove,
            };
            if let Some(t) = op.threads.iter_mut().find(|t| t.thread == tc.thread) {
                t.changes.push(change);
            } else {
                let (before, after) = if restores_snooze {
                    (tc.snooze_after, tc.snooze_before)
                } else {
                    (snooze_now, snooze_now)
                };
                op.threads.push(ThreadChange {
                    thread: tc.thread,
                    changes: vec![change],
                    snooze_before: before,
                    snooze_after: after,
                });
            }
        }
        if restores_snooze {
            ctx.tx.execute(
                "UPDATE threads SET snooze_until = ?2 WHERE id = ?1",
                params![tc.thread, tc.snooze_before],
            )?;
        }
        threads::refresh(ctx, tc.thread)?;
    }
    // A rollback after a Gmail rejection only restores local state.
    if queue {
        for op in ops {
            crate::queue::push(ctx, account, action, "modify", &op)?;
        }
    }
    Ok(())
}
