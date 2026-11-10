//! A writer on a temporary database, and builders for fetched mail.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use parking_lot::Mutex;
use sm_domain::{AccountId, Addr, GmailMessageId, GmailThreadId, LabelSet, Millis};
use sm_store::{
    Commit, FetchedThread, NewMessage, Priority, ReadPool, Reads, StoreError, WriteTx, Writer,
};
use tempfile::TempDir;

/// The writer-side state the tests' jobs and hooks see.
#[derive(Default)]
pub(crate) struct State {
    /// Tags jobs pushed, in the order they ran.
    pub order: Vec<String>,
    /// Bumped by jobs; `on_rollback` checks it was rolled back with them.
    pub dirty: u32,
}

/// A started writer, with what its hooks saw.
pub(crate) struct Harness {
    pub dir: TempDir,
    pub writer: Writer<State>,
    pub commits: Arc<Mutex<Vec<Commit>>>,
    pub rollbacks: Arc<AtomicUsize>,
}

impl Harness {
    pub(crate) fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let (writer, commits, rollbacks) = start_at(&dir.path().join("mail.db"));
        Self {
            dir,
            writer,
            commits,
            rollbacks,
        }
    }

    pub(crate) fn path(&self) -> PathBuf {
        self.dir.path().join("mail.db")
    }

    /// Stops the writer and starts a new one on the same file.
    pub(crate) fn restart(&mut self) {
        self.writer.stop().wait().unwrap();
        let (writer, commits, rollbacks) = start_at(&self.path());
        self.writer = writer;
        self.commits = commits;
        self.rollbacks = rollbacks;
    }

    /// Runs a UI job and waits for it.
    pub(crate) fn write<T: Send + 'static>(
        &self,
        job: impl FnOnce(&mut WriteTx<'_>) -> Result<T, StoreError> + Send + 'static,
    ) -> T {
        self.writer
            .call_blocking(Priority::Ui, |tx, _: &mut State| job(tx))
            .unwrap()
    }

    /// Reads on a fresh pool connection.
    pub(crate) fn read<T>(&self, read: impl FnOnce(Reads<'_>) -> Result<T, StoreError>) -> T {
        let pool = ReadPool::open(&self.path(), std::num::NonZeroUsize::MIN).unwrap();
        pool.with(|r| read(**r)).unwrap()
    }

    pub(crate) fn last_commit(&self) -> Commit {
        self.commits.lock().last().cloned().unwrap()
    }

    pub(crate) fn account(&self, email: &'static str) -> AccountId {
        self.write(move |tx| tx.accounts().create(email, "Me"))
    }
}

fn start_at(path: &std::path::Path) -> (Writer<State>, Arc<Mutex<Vec<Commit>>>, Arc<AtomicUsize>) {
    let commits: Arc<Mutex<Vec<Commit>>> = Arc::default();
    let rollbacks = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&commits);
    let rolled = Arc::clone(&rollbacks);
    let writer = Writer::start(
        path,
        State::default(),
        move |state: &mut State, snapshot, commit: &Commit| {
            // The snapshot is the committed state: its stored rev is the
            // commit's. A mismatch shows as a commit with rev -1.
            let rev = snapshot.meta().rev().ok();
            state.dirty = 0;
            seen.lock().push(if rev == Some(commit.rev) {
                commit.clone()
            } else {
                Commit {
                    rev: sm_domain::Rev(-1),
                    changes: sm_store::Changes::default(),
                }
            });
        },
        move |state: &mut State| {
            state.dirty = 0;
            rolled.fetch_add(1, Ordering::SeqCst);
        },
    )
    .unwrap();
    (writer, commits, rollbacks)
}

/// A message from `from` at `date` with these labels.
pub(crate) fn msg(gmail_id: &str, date: i64, from: &str, labels: &[&str]) -> NewMessage {
    NewMessage {
        gmail_id: GmailMessageId::from(gmail_id),
        date: Millis(date),
        from: Addr::new(from),
        to: vec![Addr::named("Me", "me@example.com")],
        cc: Vec::new(),
        bcc: Vec::new(),
        reply_to: Vec::new(),
        subject: format!("Subject of {gmail_id}"),
        snippet: format!("Snippet of {gmail_id}"),
        message_id_header: Some(format!("<{gmail_id}@example.com>")),
        in_reply_to: None,
        references: String::new(),
        is_list: false,
        is_draft: false,
        sm_draft: None,
        labels: labels.iter().copied().collect::<LabelSet>(),
        has_attachments: false,
    }
}

/// A complete fetch of a thread.
pub(crate) fn fetched(
    account: AccountId,
    gmail_id: &str,
    messages: Vec<NewMessage>,
) -> FetchedThread {
    FetchedThread {
        account,
        gmail_id: GmailThreadId::from(gmail_id),
        history_id: Some(sm_domain::HistoryId(100)),
        messages,
        complete: true,
    }
}
