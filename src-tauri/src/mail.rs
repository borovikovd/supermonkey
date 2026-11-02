//! Mail actions: archive, return to inbox, snooze, trash, spam, star, read,
//! labels (SPEC Archive, trash, star, read; Labels; Snooze; Undo).
//!
//! An action carries its resolved effect. Rust applies it to the thread's
//! non-draft messages as they are now, records exactly what changed for undo,
//! and queues one Gmail `batchModify` with those message ids.

use std::collections::BTreeSet;

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::model::*;
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
    pub snooze_before: Option<i64>,
    pub snooze_after: Option<i64>,
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

pub fn modify(ctx: &mut Ctx, n: i64, m: ModifyAction) -> anyhow::Result<()> {
    let add: BTreeSet<String> = m.add.iter().cloned().collect();
    let remove: BTreeSet<String> = m
        .remove
        .iter()
        .filter(|&l| !add.contains(l))
        .cloned()
        .collect();
    let mut changes: Vec<ThreadChange> = Vec::new();
    let mut gmail_ids: Vec<String> = Vec::new();

    for &thread in &m.threads {
        let Some((account, snooze_before)) = ctx
            .tx
            .query_row(
                "SELECT account_id, snooze_until FROM threads WHERE id = ?1",
                [thread],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?)),
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
            snooze_before,
            snooze_after: snooze_before,
            ..Default::default()
        };
        for msg in msgs.iter().filter(|x| !x.is_draft) {
            // Mark unread touches the latest message only, as Gmail does.
            let add_here: Vec<&String> = add
                .iter()
                .filter(|l| {
                    !(l.as_str() == "UNREAD"
                        && Some(msg.id) != latest_live
                        && latest_live.is_some())
                })
                .collect();
            let added: Vec<String> = add_here
                .iter()
                .filter(|l| !msg.has(l))
                .map(|l| (*l).clone())
                .collect();
            let removed: Vec<String> = remove.iter().filter(|l| msg.has(l)).cloned().collect();
            if added.is_empty() && removed.is_empty() {
                // Still part of the op: a queued action applies to the messages it recorded.
                gmail_ids.push(msg.gmail_id.clone());
                continue;
            }
            let mut labels: Vec<String> = msg
                .labels
                .iter()
                .filter(|l| !removed.contains(l))
                .cloned()
                .collect();
            labels.extend(added.iter().cloned());
            threads::set_message_labels(ctx, msg.id, &labels)?;
            gmail_ids.push(msg.gmail_id.clone());
            tc.changes.push(MsgChange {
                msg: msg.id,
                added,
                removed,
            });
        }
        // Archive, trash, and spam also clear a snooze.
        let clears = matches!(
            m.kind,
            ModifyKind::Done | ModifyKind::Trash | ModifyKind::Spam
        );
        tc.snooze_after = match &m.snooze {
            Some(SnoozeChange::Set(at)) => Some(*at),
            Some(SnoozeChange::Clear) => None,
            None if clears => None,
            None => snooze_before,
        };
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
            serde_json::json!({ "threads": changes }),
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
    ctx: &mut Ctx,
    n: i64,
    account: AccountId,
    payload: &serde_json::Value,
) -> anyhow::Result<()> {
    let threads: Vec<ThreadChange> = serde_json::from_value(payload["threads"].clone())?;
    revert(ctx, Some(n), account, &threads, true)
}

/// Shared by undo and by rollback after a Gmail rejection.
pub fn revert(
    ctx: &mut Ctx,
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
                |r| r.get::<_, Option<i64>>(0),
            )
            .optional()?
        else {
            continue;
        };
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
            match ops.iter_mut().find(|o| o.add == add && o.remove == remove) {
                Some(o) => o.messages.push(gmail_id),
                None => ops.push(ModifyOp {
                    messages: vec![gmail_id],
                    add,
                    remove,
                    threads: vec![],
                }),
            }
        }
        if snooze_now == tc.snooze_after && tc.snooze_after != tc.snooze_before {
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
