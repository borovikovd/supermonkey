//! A draft read from Gmail gets its attachments downloaded before its first
//! upsert (SPEC Addressing): `drafts.update` replaces the whole message, so
//! the upsert must carry them. Offline, the composer opens anyway, and the
//! downloads run before the queued upsert or the send.

use std::collections::HashMap;

use ub_domain::DraftId;
use ub_store::{Priority as WritePriority, StoreError};

use super::WriteFailure;
use super::write::{download, load};
use crate::workers::account::AccountCtx;

/// Downloads every attachment the draft carries that has no file yet, and
/// stores their paths on it.
pub(crate) async fn download_parts(ctx: &AccountCtx, id: DraftId) -> Result<(), WriteFailure> {
    let Some(draft) = load(&ctx.deps.pool, id.clone()).await? else {
        return Ok(());
    };
    let mut paths = HashMap::new();
    for a in &draft.attachments {
        if a.path.is_none()
            && let Some(source) = a.source
        {
            let path = download(ctx, source).await?;
            paths.insert(a.id.clone(), path.to_string_lossy().into_owned());
        }
    }
    ctx.writer()
        .call(WritePriority::Sync, move |tx, _| {
            let Some(record) = tx.read().drafts().get(&id)? else {
                return Ok::<_, StoreError>(());
            };
            let mut draft = record.draft;
            for a in &mut draft.attachments {
                if a.path.is_none() {
                    a.path = paths.get(&a.id).cloned();
                }
            }
            draft.needs_parts = false;
            tx.drafts().put(&draft)
        })
        .await
        .map_err(crate::Error::from)?;
    Ok(())
}
