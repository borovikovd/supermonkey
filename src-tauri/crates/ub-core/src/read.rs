//! Reading the database from async code: the read pool's connections block,
//! so every read runs on tokio's blocking threads, through this one helper.

use std::sync::Arc;

use ub_store::{ReadPool, ReadTx, StoreError};

use crate::Error;

/// Runs `read` in one read transaction on a pool connection, off the async
/// threads.
pub(crate) async fn read<T: Send + 'static>(
    pool: &Arc<ReadPool>,
    read: impl FnOnce(&ReadTx<'_>) -> Result<T, StoreError> + Send + 'static,
) -> Result<T, Error> {
    let pool = Arc::clone(pool);
    Ok(tokio::task::spawn_blocking(move || pool.with(read)).await??)
}
