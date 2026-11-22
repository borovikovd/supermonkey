use std::collections::VecDeque;
use std::num::NonZeroUsize;
use std::path::Path;

use parking_lot::{Condvar, Mutex};
use rusqlite::Connection;

use crate::tx::ReadTx;
use crate::{StoreError, open};

#[derive(Debug)]
pub struct ReadPool {
    free: Mutex<VecDeque<Connection>>,
    returned: Condvar,
}

impl ReadPool {
    pub fn open(path: &Path, size: NonZeroUsize) -> Result<Self, StoreError> {
        let conns = (0..size.get())
            .map(|_| open::open_reader(path))
            .collect::<Result<VecDeque<_>, _>>()?;
        Ok(Self {
            free: Mutex::new(conns),
            returned: Condvar::new(),
        })
    }

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
