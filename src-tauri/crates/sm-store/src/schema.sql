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
  send_as      TEXT NOT NULL DEFAULT '[]',
  position     INTEGER NOT NULL,
  hue          INTEGER NOT NULL,
  history_id   TEXT,
  last_sync    INTEGER,
  sync_status  TEXT NOT NULL DEFAULT 'idle',
  needs_signin INTEGER NOT NULL DEFAULT 0,
  connected    INTEGER NOT NULL DEFAULT 0
);

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
  kind            TEXT NOT NULL,
  position        INTEGER NOT NULL,
  query           TEXT NOT NULL DEFAULT '',
  enabled         INTEGER NOT NULL,
  hide_when_empty INTEGER NOT NULL
);
CREATE INDEX splits_account ON splits (account_id, position);

CREATE TABLE split_overrides (
  account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
  kind       TEXT NOT NULL,
  value      TEXT NOT NULL,
  split_id   INTEGER NOT NULL REFERENCES splits (id) ON DELETE CASCADE,
  PRIMARY KEY (account_id, kind, value)
) WITHOUT ROWID;

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
  has_live        INTEGER NOT NULL DEFAULT 0,
  in_inbox        INTEGER NOT NULL DEFAULT 0,
  snooze_until    INTEGER,
  split_id        INTEGER REFERENCES splits (id) ON DELETE SET NULL,
  history_id      TEXT,
  rev             INTEGER NOT NULL,
  UNIQUE (account_id, gmail_id)
);
CREATE INDEX threads_inbox  ON threads (account_id, in_inbox, sort_time DESC);
CREATE INDEX threads_live   ON threads (account_id, has_live, sort_time DESC);
CREATE INDEX threads_snooze ON threads (snooze_until) WHERE snooze_until IS NOT NULL;

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
  to_addrs        TEXT NOT NULL,
  cc_addrs        TEXT NOT NULL,
  bcc_addrs       TEXT NOT NULL,
  reply_to        TEXT NOT NULL,
  subject         TEXT NOT NULL,
  snippet         TEXT NOT NULL,
  message_id_hdr  TEXT,
  in_reply_to     TEXT,
  references_hdr  TEXT NOT NULL,
  is_list         INTEGER NOT NULL,
  is_draft        INTEGER NOT NULL,
  is_live         INTEGER NOT NULL,
  sm_draft_id     TEXT,
  label_ids       TEXT NOT NULL,
  has_attachments INTEGER NOT NULL,
  has_body        INTEGER NOT NULL DEFAULT 0,
  body_html       TEXT,
  quote_html      TEXT,
  body_text       TEXT,
  is_plain        INTEGER NOT NULL DEFAULT 0,
  rev             INTEGER NOT NULL,
  UNIQUE (account_id, gmail_id)
);
CREATE INDEX messages_thread ON messages (thread_id, internal_date);
CREATE INDEX messages_msgid  ON messages (account_id, message_id_hdr);
CREATE INDEX messages_draft  ON messages (account_id, sm_draft_id) WHERE sm_draft_id IS NOT NULL;

CREATE TABLE attachments (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  message_id    INTEGER NOT NULL REFERENCES messages (id) ON DELETE CASCADE,
  part_id       TEXT NOT NULL,
  filename      TEXT NOT NULL,
  mime_type     TEXT NOT NULL,
  size          INTEGER NOT NULL,
  gmail_att_id  TEXT,
  content_id    TEXT,
  is_inline     INTEGER NOT NULL,
  local_path    TEXT,
  UNIQUE (message_id, part_id)
);

CREATE VIRTUAL TABLE messages_fts USING fts5 (
  from_text, to_text, cc_text, subject, body,
  content = '', contentless_delete = 1,
  tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TABLE contacts (
  account_id   INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
  email        TEXT NOT NULL,
  name         TEXT NOT NULL,
  sent_count   INTEGER NOT NULL,
  recv_count   INTEGER NOT NULL,
  list_only    INTEGER NOT NULL,
  last_contact INTEGER NOT NULL,
  PRIMARY KEY (account_id, email)
) WITHOUT ROWID;

CREATE TABLE contact_sources (
  account_id   INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
  gmail_msg_id TEXT NOT NULL,
  PRIMARY KEY (account_id, gmail_msg_id)
) WITHOUT ROWID;

CREATE TABLE drafts (
  id                 TEXT PRIMARY KEY,
  account_id         INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
  kind               TEXT NOT NULL,
  reply_all          INTEGER NOT NULL,
  thread_id          INTEGER REFERENCES threads (id) ON DELETE SET NULL,
  target_message_id  INTEGER REFERENCES messages (id) ON DELETE SET NULL,
  started_on_latest  INTEGER NOT NULL,
  from_addr          TEXT NOT NULL,
  to_addrs           TEXT NOT NULL,
  cc_addrs           TEXT NOT NULL,
  bcc_addrs          TEXT NOT NULL,
  subject            TEXT NOT NULL,
  body_html          TEXT NOT NULL,
  snippet            TEXT NOT NULL,
  quote_html         TEXT,
  attachments        TEXT NOT NULL,
  in_reply_to        TEXT,
  references_hdr     TEXT NOT NULL,
  updated_at         INTEGER NOT NULL,
  stage              TEXT NOT NULL
                     CHECK (stage IN ('fresh', 'opened', 'kept')),
  defaults           TEXT,
  waits_for          TEXT
                     CHECK (waits_for IN ('parts', 'target_body')),
  gmail_draft_id     TEXT,
  gmail_message_id   TEXT,
  gmail_thread_id    TEXT,
  gmail_create_sent  INTEGER NOT NULL,
  gmail_change_skipped INTEGER NOT NULL,
  rev                INTEGER NOT NULL,
  CHECK ((stage = 'kept') = (defaults IS NULL))
);
CREATE INDEX drafts_account ON drafts (account_id, updated_at);
CREATE INDEX drafts_thread  ON drafts (thread_id);
CREATE INDEX drafts_gmail   ON drafts (account_id, gmail_draft_id);

CREATE TABLE outbox (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id      INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
  draft_id        TEXT NOT NULL,
  thread_id       INTEGER REFERENCES threads (id) ON DELETE SET NULL,
  send_after      INTEGER NOT NULL,
  state           TEXT NOT NULL,
  gmail_draft_id  TEXT,
  sent_message_id TEXT,
  undo_requested  INTEGER NOT NULL DEFAULT 0,
  toast_closed    INTEGER NOT NULL DEFAULT 0,
  delayed         INTEGER NOT NULL DEFAULT 0,
  error           TEXT,
  rev             INTEGER NOT NULL
);
CREATE INDEX outbox_account ON outbox (account_id);
CREATE INDEX outbox_thread  ON outbox (thread_id);
CREATE INDEX outbox_draft   ON outbox (draft_id);

CREATE TABLE ops (
  id             INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id     INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
  action         INTEGER,
  kind           TEXT NOT NULL,
  draft_id       TEXT,
  gmail_draft_id TEXT,
  payload        TEXT NOT NULL,
  created_at     INTEGER NOT NULL
);
CREATE INDEX ops_account ON ops (account_id, id);

CREATE TABLE undo_entries (
  id         INTEGER PRIMARY KEY,
  account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
  kind       TEXT NOT NULL,
  label      TEXT NOT NULL,
  outbox_id  INTEGER,
  payload    TEXT NOT NULL
);
CREATE INDEX undo_account ON undo_entries (account_id, id);

CREATE TABLE notices (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
  kind       TEXT NOT NULL,
  message    TEXT NOT NULL,
  draft_id   TEXT,
  thread_id  INTEGER,
  created_at INTEGER NOT NULL
);
CREATE INDEX notices_account ON notices (account_id, id);

CREATE TABLE settings (
  account_id INTEGER NOT NULL,
  key        TEXT NOT NULL,
  value      TEXT NOT NULL,
  PRIMARY KEY (account_id, key)
) WITHOUT ROWID;
