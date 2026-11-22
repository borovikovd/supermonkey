use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite failed to {what}")]
    Sqlite {
        what: &'static str,
        #[source]
        source: rusqlite::Error,
    },
    #[error("couldn't {action} {}", path.display())]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("couldn't encode {column} as JSON")]
    Json {
        column: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("{what} {id} not found")]
    NotFound { what: &'static str, id: String },
    #[error("{0}")]
    Invalid(String),
    #[error("the writer has stopped")]
    WriterStopped,
    #[error("a writer job panicked: {0}")]
    JobPanicked(String),
}

impl StoreError {
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
