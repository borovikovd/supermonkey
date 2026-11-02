//! Draft sync with Gmail. Filled in with the composer.

use std::sync::Arc;

use ub_gmail::GmailError;

use super::ops::Done;
use super::{AccountRt, EngineRef};
use crate::model::*;

pub async fn sync_list(_e: &EngineRef, _rt: &Arc<AccountRt>) -> Result<(), GmailError> {
    Ok(())
}

pub async fn download_parts(_e: EngineRef, _rt: Arc<AccountRt>, _draft: DraftId) {}

pub async fn read_skipped(_e: EngineRef, _rt: Arc<AccountRt>, _draft: DraftId) {}

pub async fn execute_op(
    _e: &EngineRef,
    _rt: &Arc<AccountRt>,
    _kind: &str,
    _payload: &str,
) -> Result<Done, GmailError> {
    Ok(Done::Finished)
}
