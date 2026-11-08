//! The read pool (ARCHITECTURE.md, section 4): a few read-only WAL
//! connections for window fetches, bodies, autocomplete, and search, so
//! reads never wait on the writer.

use std::collections::VecDeque;
use std::num::NonZeroUsize;
use std::path::Path;

use parking_lot::{Condvar, Mutex};
use rusqlite::Connection;

use crate::tx::ReadTx;
use crate::{StoreError, open};

/// Read-only connections, handed out round-robin: a read takes the free
/// connection that has waited longest and gives it back after. Calls block,
/// so run them off the async runtime's threads (`spawn_blocking`).
pub struct ReadPool {
    free: Mutex<VecDeque<Connection>>,
    returned: Condvar,
}

impl ReadPool {
    /// Opens `size` read-only connections to the database the writer made.
    /// Start the [`Writer`](crate::Writer) first.
    pub fn open(path: &Path, size: NonZeroUsize) -> Result<Self, StoreError> {
        let conns = (0..size.get())
            .map(|_| open::open_reader(path))
            .collect::<Result<VecDeque<_>, _>>()?;
        Ok(Self {
            free: Mutex::new(conns),
            returned: Condvar::new(),
        })
    }

    /// Runs `read` in one read transaction, so every read in it sees the same
    /// committed state. Waits only when every connection is in use. The pool's
    /// lock is not held while `read` runs.
    pub fn with<T, E: From<StoreError>>(
        &self,
        read: impl FnOnce(&ReadTx<'_>) -> Result<T, E>,
    ) -> Result<T, E> {
        let mut lease = Lease {
            pool: self,
            conn: Some(self.take()),
        };
        let conn = lease.conn.as_mut().ok_or(StoreError::Invalid(
            "a read connection went missing".to_owned(),
        ))?;
        // Deferred: a read-only transaction that pins one snapshot. It rolls
        // back on drop, which for reads is the same as a commit.
        let tx = conn
            .transaction()
            .map_err(StoreError::sql("begin a read transaction"))?;
        read(&ReadTx::new(&tx))
    }

    fn take(&self) -> Connection {
        let mut free = self.free.lock();
        loop {
            if let Some(conn) = free.pop_front() {
                return conn;
            }
            self.returned.wait(&mut free);
        }
    }
}

/// A connection out of the pool; goes back when dropped, panics included.
struct Lease<'p> {
    pool: &'p ReadPool,
    conn: Option<Connection>,
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            self.pool.free.lock().push_back(conn);
            self.pool.returned.notify_one();
        }
    }
}
