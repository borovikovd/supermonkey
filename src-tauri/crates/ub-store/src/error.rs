//! The store's one error type.

use std::path::PathBuf;

/// Why a store call failed.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// SQLite refused a statement. `what` names the read or write.
    #[error("sqlite failed to {what}")]
    Sqlite {
        /// The read or write that failed, as "read a thread row".
        what: &'static str,
        /// SQLite's error.
        #[source]
        source: rusqlite::Error,
    },
    /// The database file or its folder could not be prepared.
    #[error("couldn't {action} {}", path.display())]
    Io {
        /// What was being done, as "create the folder".
        action: &'static str,
        /// The file or folder.
        path: PathBuf,
        /// The OS error.
        #[source]
        source: std::io::Error,
    },
    /// A value could not be written as JSON for a column.
    #[error("couldn't encode {column} as JSON")]
    Json {
        /// The column.
        column: &'static str,
        /// The encoder's error.
        #[source]
        source: serde_json::Error,
    },
    /// A write named a row that does not exist.
    #[error("{what} {id} not found")]
    NotFound {
        /// The kind of row, as "draft".
        what: &'static str,
        /// Its id.
        id: String,
    },
    /// A write would break a rule of the store, such as moving a draft to
    /// another account.
    #[error("{0}")]
    Invalid(String),
    /// The writer thread is gone, or stopped before it ran the job.
    #[error("the writer has stopped")]
    WriterStopped,
    /// A writer job panicked. Its transaction rolled back.
    #[error("a writer job panicked: {0}")]
    JobPanicked(String),
    /// `call_blocking` on the writer thread itself, which would wait forever.
    #[error("a writer job can't wait for another writer job")]
    Reentrant,
}

impl StoreError {
    /// A closure that wraps a SQLite error with what was being done.
    pub(crate) fn sql(what: &'static str) -> impl FnOnce(rusqlite::Error) -> Self {
        move |source| Self::Sqlite { what, source }
    }

    pub(crate) fn not_found(what: &'static str, id: &dyn std::fmt::Display) -> Self {
        Self::NotFound {
            what,
            id: id.to_string(),
        }
    }
}
