use std::collections::HashMap;

use sm_domain::DraftId;
use sm_store::{Priority as WritePriority, StoreError};

use super::WriteFailure;
use super::write::{download, load};
use crate::drafts::lifecycle;
use crate::workers::account::AccountCtx;

pub(crate) async fn download_parts(ctx: &AccountCtx, id: DraftId) -> Result<(), WriteFailure> {
    let Some(record) = load(&ctx.deps.pool, id.clone()).await? else {
        return Ok(());
    };
    let draft = record.draft;
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
                    a.path = paths.remove(&a.id);
                }
            }
            lifecycle::parts_downloaded(&mut draft);
            tx.drafts().put(&draft)
        })
        .await
        .map_err(crate::Error::from)?;
    Ok(())
}
