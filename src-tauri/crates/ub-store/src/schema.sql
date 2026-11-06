-- Ubermensch's mail store (SPEC Data). One file in the app data directory,
-- created fresh: a file with another `user_version` is moved aside, never
-- migrated (ARCHITECTURE.md, section 6).
--
-- - Rows are keyed by account, so two Gmail accounts never collide.
-- - `rev` columns hold the write transaction that last changed the row.
-- - Ids the UI, the op queue, or the undo history hold use AUTOINCREMENT, so a
--   late message about a deleted row can never reach a new one (AGENTS.md,
--   Concurrency).
-- - JSON columns hold arrays of `{"name", "email"}` mailboxes or label ids.

-- One row: the last `rev` committed and the last action number applied.
CREATE TABLE meta (
  id          INTEGER PRIMARY KEY CHECK (id = 1),
  rev         INTEGER NOT NULL,
  last_action INTEGER NOT NULL
);
INSERT INTO meta (id, rev, last_action) VALUES (1, 0, 0);

CREATE TABLE accounts (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  email        TEXT NOT NULL UNIQUE COLLATE NOCASE,
  display_name TEXT NOT NULL,
  send_as      TEXT NOT NULL DEFAULT '[]',     -- JSON [{email, name, is_default, is_primary}]
  position     INTEGER NOT NULL,               -- rail order
  hue          INTEGER NOT NULL,               -- avatar hue slot
  history_id   TEXT,                           -- Gmail's history cursor
  last_sync    INTEGER,
  sync_status  TEXT NOT NULL DEFAULT 'idle',   -- SyncStatus
  needs_signin INTEGER NOT NULL DEFAULT 0,
  connected    INTEGER NOT NULL DEFAULT 0      -- the connect sync stored its cursor
);

-- `id` is the local id used everywhere here: Gmail's id for labels Gmail
-- made, the UI's for one made here until Gmail assigns `gmail_id`.
CREATE TABLE labels (
  account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
  id         TEXT NOT NULL,
  gmail_id   TEXT,
  name       TEXT NOT NULL,
  system     INTEGER NOT NULL,
  PRIMARY KEY (account_id, id)
) WITHOUT ROWID;
CREATE UNIQUE INDEX labels_gmail ON labels (account_id, gmail_id);

CREATE TABLE splits (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id      INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
  name            TEXT NOT NULL,
  kind            TEXT NOT NULL,               -- SplitKind
  position        INTEGER NOT NULL,
  query           TEXT NOT NULL DEFAULT '',
  enabled         INTEGER NOT NULL,
  hide_when_empty INTEGER NOT NULL
);
CREATE INDEX splits_account ON splits (account_id, position);

-- Pins: a sender, domain, or label name routed to a split.
CREATE TABLE split_overrides (
  account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
  kind       TEXT NOT NULL,                    -- PinKind
  value      TEXT NOT NULL,                    -- lowercased
  split_id   INTEGER NOT NULL REFERENCES splits (id) ON DELETE CASCADE,
  PRIMARY KEY (account_id, kind, value)
) WITHOUT ROWID;

-- A thread's row is derived from its messages by `ub_domain::Thread::digest`
-- whenever one of them is written; it goes when the last message goes.
CREATE TABLE threads (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id      INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
  gmail_id        TEXT NOT NULL,
  subject         TEXT NOT NULL DEFAULT '',
  snippet         TEXT NOT NULL DEFAULT '',
  sender          TEXT NOT NULL DEFAULT '',
  sender_email    TEXT NOT NULL DEFAULT '',
  sort_time       INTEGER NOT NULL DEFAULT 0,
  message_count   INTEGER NOT NULL DEFAULT 0,
  unread          INTEGER NOT NULL DEFAULT 0,
  has_attachments INTEGER NOT NULL DEFAULT 0,
  has_live        INTEGER NOT NULL DEFAULT 0,  -- a message not a draft and not in trash or spam
  in_inbox        INTEGER NOT NULL DEFAULT 0,
  snooze_until    INTEGER,
  split_id        INTEGER REFERENCES splits (id) ON DELETE SET NULL,
  history_id      TEXT,                        -- Gmail's historyId at the last fetch
  rev             INTEGER NOT NULL,
  UNIQUE (account_id, gmail_id)
);
CREATE INDEX threads_inbox  ON threads (account_id, in_inbox, sort_time DESC);
CREATE INDEX threads_live   ON threads (account_id, has_live, sort_time DESC);
CREATE INDEX threads_snooze ON threads (snooze_until) WHERE snooze_until IS NOT NULL;

-- The labels a thread is in, by the rule in SPEC Views.
CREATE TABLE thread_labels (
  thread_id  INTEGER NOT NULL REFERENCES threads (id) ON DELETE CASCADE,
  account_id INTEGER NOT NULL,
  label_id   TEXT NOT NULL,
  PRIMARY KEY (thread_id, label_id)
) WITHOUT ROWID;
CREATE INDEX thread_labels_label ON thread_labels (account_id, label_id);

CREATE TABLE messages (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id      INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
  thread_id       INTEGER NOT NULL REFERENCES threads (id) ON DELETE CASCADE,
  gmail_id        TEXT NOT NULL,
  internal_date   INTEGER NOT NULL,
  from_name       TEXT,
  from_email      TEXT NOT NULL,
  to_addrs        TEXT NOT NULL,               -- JSON
  cc_addrs        TEXT NOT NULL,               -- JSON
  bcc_addrs       TEXT NOT NULL,               -- JSON
  reply_to        TEXT NOT NULL,               -- JSON
  subject         TEXT NOT NULL,
  snippet         TEXT NOT NULL,
  message_id_hdr  TEXT,                        -- Message-ID
  in_reply_to     TEXT,
  references_hdr  TEXT NOT NULL,
  is_list         INTEGER NOT NULL,            -- has List-Id or List-Unsubscribe
  is_draft        INTEGER NOT NULL,
  is_live         INTEGER NOT NULL,            -- not a draft, not in trash or spam
  ub_draft_id     TEXT,                        -- X-Ubermensch-Draft
  label_ids       TEXT NOT NULL,               -- JSON, local label ids
  has_attachments INTEGER NOT NULL,
  has_body        INTEGER NOT NULL DEFAULT 0,
  body_html       TEXT,                        -- sanitized; NULL for plain text mail
  quote_html      TEXT,                        -- the quoted history, sanitized
  body_text       TEXT,
  is_plain        INTEGER NOT NULL DEFAULT 0,
  rev             INTEGER NOT NULL,
  UNIQUE (account_id, gmail_id)
);
CREATE INDEX messages_thread ON messages (thread_id, internal_date);
CREATE INDEX messages_msgid  ON messages (account_id, message_id_hdr);
CREATE INDEX messages_draft  ON messages (account_id, ub_draft_id) WHERE ub_draft_id IS NOT NULL;

CREATE TABLE attachments (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  message_id    INTEGER NOT NULL REFERENCES messages (id) ON DELETE CASCADE,
  part_id       TEXT NOT NULL,
  filename      TEXT NOT NULL,
  mime_type     TEXT NOT NULL,
  size          INTEGER NOT NULL,
  gmail_att_id  TEXT,
  content_id    TEXT,                          -- Content-ID of an inline part
  is_inline     INTEGER NOT NULL,
  local_path    TEXT,                          -- once downloaded
  UNIQUE (message_id, part_id)
);

-- Full-text search over FTS_COLUMNS (ub_domain::query). Contentless: the
-- text already lives in `messages`, so the index stores only its tokens.
-- The rowid is the message id.
CREATE VIRTUAL TABLE messages_fts USING fts5 (
  from_text, to_text, cc_text, subject, body,
  content = '', contentless_delete = 1,
  tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TABLE contacts (
  account_id   INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
  email        TEXT NOT NULL,                  -- lowercased
  name         TEXT NOT NULL,
  sent_count   INTEGER NOT NULL,
  recv_count   INTEGER NOT NULL,
  list_only    INTEGER NOT NULL,
  last_contact INTEGER NOT NULL,
  PRIMARY KEY (account_id, email)
) WITHOUT ROWID;

-- Every Gmail message already counted into `contacts`.
CREATE TABLE contact_sources (
  account_id   INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
  gmail_msg_id TEXT NOT NULL,
  PRIMARY KEY (account_id, gmail_msg_id)
) WITHOUT ROWID;

CREATE TABLE drafts (
  id                 TEXT PRIMARY KEY,         -- local id, made by the UI
  account_id         INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
  kind               TEXT NOT NULL,            -- DraftKind
  reply_all          INTEGER NOT NULL,
  thread_id          INTEGER REFERENCES threads (id) ON DELETE SET NULL,
  target_message_id  INTEGER REFERENCES messages (id) ON DELETE SET NULL,
  started_on_latest  INTEGER NOT NULL,
  started_fresh      INTEGER NOT NULL,
  from_addr          TEXT NOT NULL,            -- JSON mailbox or null
  to_addrs           TEXT NOT NULL,            -- JSON
  cc_addrs           TEXT NOT NULL,            -- JSON
  bcc_addrs          TEXT NOT NULL,            -- JSON
  subject            TEXT NOT NULL,
  body_html          TEXT NOT NULL,
  snippet            TEXT NOT NULL,            -- the body as text, for list rows
  quote_html         TEXT,                     -- the kept quote block
  attachments        TEXT NOT NULL,            -- JSON [DraftAttachment]
  defaults           TEXT,                     -- JSON DraftContent the composer started with
  in_reply_to        TEXT,
  references_hdr     TEXT NOT NULL,
  gmail_draft_id     TEXT,
  gmail_message_id   TEXT,
  gmail_thread_id    TEXT,
  pending_gmail_read INTEGER NOT NULL,         -- a Gmail change skipped while a composer had it open
  needs_parts        INTEGER NOT NULL,         -- DraftNeed: 0 none, 1 parts (read from Gmail), 2 target's body
  gmail_create_sent  INTEGER NOT NULL,         -- a drafts.create went out; its answer is not stored
  updated_at         INTEGER NOT NULL,
  rev                INTEGER NOT NULL
);
CREATE INDEX drafts_account ON drafts (account_id, updated_at);
CREATE INDEX drafts_thread  ON drafts (thread_id);
CREATE INDEX drafts_gmail   ON drafts (account_id, gmail_draft_id);

-- The UI keys send toasts by id.
CREATE TABLE outbox (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id      INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
  draft_id        TEXT NOT NULL,
  thread_id       INTEGER REFERENCES threads (id) ON DELETE SET NULL,
  send_after      INTEGER NOT NULL,
  state           TEXT NOT NULL,               -- OutboxState
  gmail_draft_id  TEXT,
  sent_message_id TEXT,
  undo_requested  INTEGER NOT NULL DEFAULT 0,  -- a held undo, applied when the send resolves
  toast_closed    INTEGER NOT NULL DEFAULT 0,
  delayed         INTEGER NOT NULL DEFAULT 0,
  error           TEXT,
  rev             INTEGER NOT NULL
);
CREATE INDEX outbox_account ON outbox (account_id);
CREATE INDEX outbox_thread  ON outbox (thread_id);
CREATE INDEX outbox_draft   ON outbox (draft_id);

-- Queued Gmail mutations, replayed in id order per account. An op deleted
-- while it runs must not lend its id to a new op its runner would delete.
CREATE TABLE ops (
  id             INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id     INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
  action         INTEGER,                      -- the UI action that queued it
  kind           TEXT NOT NULL,                -- OpKind
  draft_id       TEXT,                         -- the local draft it concerns
  gmail_draft_id TEXT,                         -- the Gmail draft it concerns
  payload        TEXT NOT NULL,                -- JSON, the caller's
  created_at     INTEGER NOT NULL
);
CREATE INDEX ops_account ON ops (account_id, id);

-- The last 50 undo entries per account. The id is the number of the action
-- that made it, and action numbers never repeat.
CREATE TABLE undo_entries (
  id         INTEGER PRIMARY KEY,
  account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
  kind       TEXT NOT NULL,                    -- UndoKind
  label      TEXT NOT NULL,                    -- what the toast says
  outbox_id  INTEGER,                          -- a send entry's row, to find it by
  payload    TEXT NOT NULL                     -- JSON UndoAction
);
CREATE INDEX undo_account ON undo_entries (account_id, id);

-- The UI shows each notice id once.
CREATE TABLE notices (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
  kind       TEXT NOT NULL,                    -- NoticeKind
  message    TEXT NOT NULL,
  draft_id   TEXT,
  thread_id  INTEGER,
  created_at INTEGER NOT NULL
);
CREATE INDEX notices_account ON notices (account_id, id);

-- Account 0 holds the global settings.
CREATE TABLE settings (
  account_id INTEGER NOT NULL,
  key        TEXT NOT NULL,
  value      TEXT NOT NULL,
  PRIMARY KEY (account_id, key)
) WITHOUT ROWID;
