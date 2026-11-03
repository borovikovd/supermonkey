-- Ubermensch mail store. One file in the app data directory.
-- Every row is keyed by account so two Gmail accounts never collide.
-- `rev` columns hold the writer transaction that last changed the row.

PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS meta (
  key   TEXT PRIMARY KEY,
  value INTEGER NOT NULL
);
INSERT OR IGNORE INTO meta (key, value) VALUES ('rev', 0), ('last_action', 0), ('schema', 1);

CREATE TABLE IF NOT EXISTS accounts (
  id            INTEGER PRIMARY KEY,
  email         TEXT NOT NULL UNIQUE,
  display_name  TEXT NOT NULL DEFAULT '',
  send_as       TEXT NOT NULL DEFAULT '[]',   -- JSON [SendAs]
  position      INTEGER NOT NULL,
  hue           INTEGER NOT NULL,
  history_id    TEXT,
  last_sync     INTEGER,
  needs_signin  INTEGER NOT NULL DEFAULT 0,
  sync_status   TEXT NOT NULL DEFAULT 'idle',  -- idle | syncing | offline | retrying | error
  connected     INTEGER NOT NULL DEFAULT 0,    -- connect sync has stored history_id
  rev           INTEGER NOT NULL DEFAULT 0
);

-- `id` is the stable key used everywhere in this database. For labels that came
-- from Gmail it equals `gmail_id`. A label created here gets a local id from the
-- UI, and `gmail_id` is filled when the create replays.
CREATE TABLE IF NOT EXISTS labels (
  account_id  INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  id          TEXT NOT NULL,
  gmail_id    TEXT,
  name        TEXT NOT NULL,
  system      INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (account_id, id)
);
CREATE UNIQUE INDEX IF NOT EXISTS labels_gmail ON labels(account_id, gmail_id);

CREATE TABLE IF NOT EXISTS threads (
  id              INTEGER PRIMARY KEY,
  account_id      INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  gmail_id        TEXT NOT NULL,
  subject         TEXT NOT NULL DEFAULT '',
  snippet         TEXT NOT NULL DEFAULT '',
  sender_name     TEXT NOT NULL DEFAULT '',
  sender_email    TEXT NOT NULL DEFAULT '',
  sort_time       INTEGER NOT NULL DEFAULT 0,
  message_count   INTEGER NOT NULL DEFAULT 0,
  unread          INTEGER NOT NULL DEFAULT 0,
  starred         INTEGER NOT NULL DEFAULT 0,
  has_attachments INTEGER NOT NULL DEFAULT 0,
  in_inbox        INTEGER NOT NULL DEFAULT 0,
  in_trash        INTEGER NOT NULL DEFAULT 0,
  in_spam         INTEGER NOT NULL DEFAULT 0,
  has_live        INTEGER NOT NULL DEFAULT 0,  -- a message that is not a draft and not in trash or spam
  snooze_until    INTEGER,
  split_id        INTEGER,
  history_id      TEXT,
  rev             INTEGER NOT NULL DEFAULT 0,
  UNIQUE (account_id, gmail_id)
);
CREATE INDEX IF NOT EXISTS threads_inbox  ON threads(account_id, in_inbox, sort_time DESC);
CREATE INDEX IF NOT EXISTS threads_sort   ON threads(account_id, sort_time DESC);
CREATE INDEX IF NOT EXISTS threads_snooze ON threads(snooze_until) WHERE snooze_until IS NOT NULL;

-- Labels a thread is in, by the rule in SPEC Views.
CREATE TABLE IF NOT EXISTS thread_labels (
  thread_id  INTEGER NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
  label_id   TEXT NOT NULL,
  PRIMARY KEY (thread_id, label_id)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS thread_labels_label ON thread_labels(label_id, thread_id);

CREATE TABLE IF NOT EXISTS messages (
  id              INTEGER PRIMARY KEY,
  account_id      INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  thread_id       INTEGER NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
  gmail_id        TEXT NOT NULL,
  internal_date   INTEGER NOT NULL DEFAULT 0,
  from_name       TEXT NOT NULL DEFAULT '',
  from_email      TEXT NOT NULL DEFAULT '',
  to_json         TEXT NOT NULL DEFAULT '[]',
  cc_json         TEXT NOT NULL DEFAULT '[]',
  bcc_json        TEXT NOT NULL DEFAULT '[]',
  reply_to_json   TEXT NOT NULL DEFAULT '[]',
  subject         TEXT NOT NULL DEFAULT '',
  snippet         TEXT NOT NULL DEFAULT '',
  message_id_hdr  TEXT,
  in_reply_to     TEXT,
  references_hdr  TEXT NOT NULL DEFAULT '',
  is_list         INTEGER NOT NULL DEFAULT 0,
  is_draft        INTEGER NOT NULL DEFAULT 0,
  ub_draft_id     TEXT,
  label_ids       TEXT NOT NULL DEFAULT '[]',   -- JSON, local label ids
  has_body        INTEGER NOT NULL DEFAULT 0,
  body_html       TEXT,                         -- sanitized
  quote_html      TEXT,                         -- quoted history, sanitized
  body_text       TEXT,
  is_plain        INTEGER NOT NULL DEFAULT 0,
  has_attachments INTEGER NOT NULL DEFAULT 0,
  rev             INTEGER NOT NULL DEFAULT 0,
  UNIQUE (account_id, gmail_id)
);
CREATE INDEX IF NOT EXISTS messages_thread ON messages(thread_id, internal_date);
CREATE INDEX IF NOT EXISTS messages_msgid  ON messages(account_id, message_id_hdr);

CREATE TABLE IF NOT EXISTS attachments (
  id             INTEGER PRIMARY KEY,
  message_id     INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  part_id        TEXT NOT NULL,
  filename       TEXT NOT NULL,
  mime_type      TEXT NOT NULL,
  size           INTEGER NOT NULL,
  gmail_att_id   TEXT,
  content_id     TEXT,
  inline         INTEGER NOT NULL DEFAULT 0,
  local_path     TEXT
);
CREATE INDEX IF NOT EXISTS attachments_message ON attachments(message_id);

CREATE TABLE IF NOT EXISTS contacts (
  account_id    INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  email         TEXT NOT NULL,          -- lowercased
  name          TEXT NOT NULL DEFAULT '',
  sent_count    INTEGER NOT NULL DEFAULT 0,
  recv_count    INTEGER NOT NULL DEFAULT 0,
  list_only     INTEGER NOT NULL DEFAULT 1,
  last_contact  INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (account_id, email)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS contact_sources (
  account_id   INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  gmail_msg_id TEXT NOT NULL,
  PRIMARY KEY (account_id, gmail_msg_id)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS drafts (
  id                 TEXT PRIMARY KEY,           -- local id, made by the UI
  account_id         INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  kind               TEXT NOT NULL,              -- new | reply | forward
  reply_all          INTEGER NOT NULL DEFAULT 0,
  thread_id          INTEGER REFERENCES threads(id) ON DELETE SET NULL,
  target_message_id  INTEGER REFERENCES messages(id) ON DELETE SET NULL,
  started_on_latest  INTEGER NOT NULL DEFAULT 0,
  started_fresh      INTEGER NOT NULL DEFAULT 0,
  from_json          TEXT NOT NULL DEFAULT 'null',
  to_json            TEXT NOT NULL DEFAULT '[]',
  cc_json            TEXT NOT NULL DEFAULT '[]',
  bcc_json           TEXT NOT NULL DEFAULT '[]',
  subject            TEXT NOT NULL DEFAULT '',
  body_html          TEXT NOT NULL DEFAULT '',
  quote_html         TEXT,
  attachments_json   TEXT NOT NULL DEFAULT '[]', -- [DraftAttachment]
  defaults_json      TEXT,                       -- fields as the composer started them
  in_reply_to        TEXT,
  references_hdr     TEXT NOT NULL DEFAULT '',
  gmail_draft_id     TEXT,
  gmail_message_id   TEXT,
  gmail_thread_id    TEXT,
  pending_gmail_read INTEGER NOT NULL DEFAULT 0,  -- a Gmail change skipped while a composer had it open
  needs_parts        INTEGER NOT NULL DEFAULT 0,  -- read from Gmail; parts not downloaded yet
  updated_at         INTEGER NOT NULL DEFAULT 0,
  rev                INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS drafts_thread ON drafts(thread_id);
CREATE INDEX IF NOT EXISTS drafts_gmail ON drafts(account_id, gmail_draft_id);

-- AUTOINCREMENT: the UI keys send toasts by id, so an id is never reused.
CREATE TABLE IF NOT EXISTS outbox (
  id                INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id        INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  draft_id          TEXT NOT NULL,
  thread_id         INTEGER,
  send_after        INTEGER NOT NULL,
  state             TEXT NOT NULL,              -- waiting | sending | sent
  gmail_draft_id    TEXT,
  sent_message_id   TEXT,
  undo_requested    INTEGER NOT NULL DEFAULT 0,
  toast_closed      INTEGER NOT NULL DEFAULT 0,
  delayed           INTEGER NOT NULL DEFAULT 0,
  error             TEXT,
  rev               INTEGER NOT NULL DEFAULT 0
);

-- Gmail mutations waiting to run, in order per account.
-- AUTOINCREMENT: an op deleted while it runs (by send, discard, or a newer
-- save) must not lend its id to a new op, which its runner would then delete.
CREATE TABLE IF NOT EXISTS ops (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id  INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  action      INTEGER,               -- UI action number that queued it, if any
  kind        TEXT NOT NULL,         -- modify | label_create | draft_upsert | draft_delete | trash_message
  payload     TEXT NOT NULL,         -- JSON
  attempts    INTEGER NOT NULL DEFAULT 0,
  created_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS ops_account ON ops(account_id, id);

-- Undo history, the last 50 per account. `id` is the action number that made it.
CREATE TABLE IF NOT EXISTS undo_entries (
  id          INTEGER PRIMARY KEY,
  account_id  INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  kind        TEXT NOT NULL,         -- mail | send | pin
  label       TEXT NOT NULL,         -- what the toast says, e.g. "Marked done"
  payload     TEXT NOT NULL          -- JSON
);
CREATE INDEX IF NOT EXISTS undo_account ON undo_entries(account_id, id);

-- AUTOINCREMENT: ids are never reused, because the UI shows each id once.
CREATE TABLE IF NOT EXISTS notices (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id  INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  kind        TEXT NOT NULL,         -- rejected | send_failed | send_undone | draft_gone | info
  message     TEXT NOT NULL,
  draft_id    TEXT,
  thread_id   INTEGER,
  created_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS splits (
  id              INTEGER PRIMARY KEY,
  account_id      INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  name            TEXT NOT NULL,
  position        INTEGER NOT NULL,
  query           TEXT NOT NULL DEFAULT '',
  hide_when_empty INTEGER NOT NULL DEFAULT 1,
  enabled         INTEGER NOT NULL DEFAULT 1,
  kind            TEXT NOT NULL DEFAULT 'custom'   -- custom | important | other
);

CREATE TABLE IF NOT EXISTS split_overrides (
  account_id  INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  kind        TEXT NOT NULL,          -- sender | domain | label
  value       TEXT NOT NULL,          -- lowercased email, domain, or label name
  split_id    INTEGER NOT NULL REFERENCES splits(id) ON DELETE CASCADE,
  PRIMARY KEY (account_id, kind, value)
);

CREATE TABLE IF NOT EXISTS settings (
  account_id  INTEGER NOT NULL DEFAULT 0,   -- 0 = global
  key         TEXT NOT NULL,
  value       TEXT NOT NULL,
  PRIMARY KEY (account_id, key)
);

CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
  from_text, to_text, cc_text, subject, body,
  tokenize = 'unicode61 remove_diacritics 2'
);
