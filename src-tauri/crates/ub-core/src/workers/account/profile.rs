//! The account's `sendAs` addresses and labels, read on connect and on focus
//! (SPEC Sync).

use std::collections::BTreeMap;

use ub_domain::{LabelId, SendAsAddress};
use ub_gmail::{GmailError, LabelType, Priority};
use ub_store::{GmailLabel, Priority as WritePriority, StoreError};

use super::ctx::AccountCtx;

/// Reads `sendAs` and stores every address; the display name comes from the
/// primary entry. Writes only when something changed.
pub(crate) async fn refresh_send_as(ctx: &AccountCtx) -> Result<(), GmailError> {
    let listed = ctx.gmail.list_send_as(Priority::P3).await?;
    let send_as: Vec<SendAsAddress> = listed
        .into_iter()
        .map(|s| SendAsAddress {
            email: s.send_as_email,
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
        move |tx, _| {
            let Some(record) = tx.read().accounts().get(account)? else {
                return Ok(());
            };
            let name = name.unwrap_or(record.display_name.clone());
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
/// changed.
///
/// Extension point: the labels slice keeps a label whose Gmail id was
/// assigned after this list was read (a `labels.create` that landed
/// meanwhile), which the list cannot know yet.
pub(crate) async fn refresh_labels(ctx: &AccountCtx) -> Result<(), GmailError> {
    let listed = ctx.gmail.list_labels(Priority::P3).await?;
    let labels: Vec<GmailLabel> = listed
        .into_iter()
        .map(|l| GmailLabel {
            system: l.label_type == Some(LabelType::System),
            gmail_id: l.id,
            name: l.name,
        })
        .collect();
    let account = ctx.account;
    ctx.writer()
        .queue(WritePriority::Sync, "store the label list", move |tx, _| {
            if tx.read().accounts().get(account)?.is_none() {
                return Ok(());
            }
            let stored: BTreeMap<LabelId, (String, bool)> = tx
                .read()
                .labels()
                .all(account)?
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
        });
    Ok(())
}
