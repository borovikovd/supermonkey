use std::sync::Arc;

use sm_store::{ReadPool, ReadTx, StoreError};

use crate::Error;

pub(crate) async fn read<T: Send + 'static>(
    pool: &Arc<ReadPool>,
    read: impl FnOnce(&ReadTx<'_>) -> Result<T, StoreError> + Send + 'static,
) -> Result<T, Error> {
    let pool = Arc::clone(pool);
    Ok(tokio::task::spawn_blocking(move || pool.with(read)).await??)
}
