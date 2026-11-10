//! The account's `sendAs` addresses and labels, read on connect and on focus
//! (SPEC Sync).

use std::collections::BTreeMap;
use std::sync::Arc;

use sm_domain::{LabelId, SendAsAddress};
use sm_gmail::{GmailError, LabelType, Priority};
use sm_store::{GmailLabel, Priority as WritePriority, StoreError};

use super::ctx::AccountCtx;

/// Reads `sendAs` and stores every address; the display name comes from the
/// primary entry. Writes only when something changed, and never over the
/// answer of a read that started later.
pub(crate) async fn refresh_send_as(ctx: &AccountCtx) -> Result<(), GmailError> {
    let lane = Arc::clone(&ctx.lane);
    let read = lane.send_as_reads.start();
    let listed = ctx.gmail.list_send_as(Priority::P3).await?;
    let send_as: Vec<SendAsAddress> = listed
        .into_iter()
        .map(|s| SendAsAddress {
            email: s.email,
            name: s.display_name,
            is_default: s.is_default,
            is_primary: s.is_primary,
        })
        .collect();
    let name = send_as
        .iter()
        .find(|s| s.is_primary)
        .map(|s| s.name.clone())
        .filter(|n| !n.is_empty());
    let account = ctx.account;
    ctx.writer().queue(
        WritePriority::Sync,
        "store sendAs addresses",
        move |tx, state| {
            if !lane.send_as_reads.store(read, state) {
                return Ok(());
            }
            let Some(record) = tx.read().accounts().get(account)? else {
                return Ok(());
            };
            let name = name.unwrap_or_else(|| record.display_name.clone());
            if record.send_as != send_as || record.display_name != name {
                tx.accounts().set_profile(account, &name, &send_as)?;
            }
            Ok(())
        },
    );
    Ok(())
}

/// Reads `labels.list` and makes the account's labels match it: renames,
/// new labels, and labels deleted on Gmail. Writes only when something
/// changed. When a label made here got its Gmail id after the list was
/// read (LAB-08), no label made here is dropped: the list may not know it
/// yet, and the next refresh settles it. A list read before another that
/// was stored first is dropped.
pub(crate) async fn refresh_labels(ctx: &AccountCtx) -> Result<(), GmailError> {
    let lane = Arc::clone(&ctx.lane);
    let mapped = lane.labels_mapped();
    let read = lane.label_reads.start();
    let listed = ctx.gmail.list_labels(Priority::P3).await?;
    let mut labels: Vec<GmailLabel> = listed
        .into_iter()
        .map(|l| GmailLabel {
            system: l.kind == Some(LabelType::System),
            gmail_id: l.id,
            name: l.name,
        })
        .collect();
    let account = ctx.account;
    ctx.writer().queue(
        WritePriority::Sync,
        "store the label list",
        move |tx, state| {
            if !lane.label_reads.store(read, state) || tx.read().accounts().get(account)?.is_none()
            {
                return Ok(());
            }
            let all = tx.read().labels().all(account)?;
            if lane.labels_mapped() != mapped {
                // Labels made here carry the UI's id, not Gmail's.
                for label in &all {
                    if let Some(gmail_id) = &label.gmail_id
                        && *gmail_id != label.id
                        && !labels.iter().any(|l| l.gmail_id == *gmail_id)
                    {
                        labels.push(GmailLabel {
                            gmail_id: gmail_id.clone(),
                            name: label.name.clone(),
                            system: label.system,
                        });
                    }
                }
            }
            let stored: BTreeMap<LabelId, (String, bool)> = all
                .into_iter()
                .filter_map(|l| Some((l.gmail_id?, (l.name, l.system))))
                .collect();
            let listed: BTreeMap<LabelId, (String, bool)> = labels
                .iter()
                .map(|l| (l.gmail_id.clone(), (l.name.clone(), l.system)))
                .collect();
            if stored != listed {
                tx.labels().sync_from_gmail(account, &labels)?;
            }
            Ok::<_, StoreError>(())
        },
    );
    Ok(())
}
