# Deviations from SPEC.md

Choices made where the spec was silent, ambiguous, or could not be built as
written. Each keeps the spec's intent.

## Build and tooling

- **Workspace crates.** Pure logic lives in separate crates under
  `src-tauri/crates/`: `ub-mime` (MIME parse, sanitize, build), `ub-gmail`
  (Gmail client, OAuth, quota scheduler, Keychain), `ub-fake-gmail` (the fake
  Gmail server for tests), `ub-rules` (rule/search language, snooze times,
  contact ranking).
- **Debug web bridge.** Debug builds started with `UBERMENSCH_WEB_BRIDGE=1`
  serve the same command surface on `127.0.0.1:1430`, so the UI can run in a
  browser against the real Rust core for automated checks. Release builds
  never start it.
- **Perf measurements** were taken on this Mac in Chrome through the web
  bridge, not on an M1 MacBook Air in WKWebView.

## Data

- Undo history is its own table, `undo_entries`, rather than rows in `ops`.
  Notices that persist until the UI acks them live in `notices`.
- Labels created locally keep their local id everywhere; `labels.gmail_id`
  maps it to the Gmail id once the create replays, so nothing is rewritten.

## Gmail

- Quota costs follow Google's published table where it differs from the
  spec's list: `messages.trash` 20, `attachments.get` 20, `drafts.update` 15.
- `u` (mark unread) adds `UNREAD` to the thread's latest message only, as
  Gmail does. Mark read removes it from every message.

## UI

- Search treats `AND`/`OR` case-insensitively for local matching.

## Sync (step 2)

- History polls apply each message's latest `labelIds` from the history
  records directly, and fetch thread metadata only for messages the app does
  not know yet. Pending ops are re-applied on top, so a queued action wins.
- The snooze rule runs whenever a thread is stored from Gmail (poll, resync,
  open), skipping messages with an op still pending. That is equivalent to
  judging net labels after the batch.
- After a restart the app re-lists Inbox (unchanged threads are skipped by
  historyId) and fetches any bodies the connect sync had not reached.
- Notification clicks: the macOS notification plugin does not report clicks,
  so a click only brings the app forward. See Notifications (step 6).

## Compose (step 3)

- Attachments are picked with the webview's file input and copied into the
  draft's folder by an `add_attachment` command, so no dialog plugin is added.
- One `draft_upsert` op per draft at a time; it writes the draft's latest
  state when it runs, so fast typing does not queue a write per save.
- Undo of a waiting send clears the draft's defaults, so the reopened draft is
  never discarded as untouched.
- Demo mode: debug builds with `--features fake` and `UBERMENSCH_FAKE=1`
  (`pnpm demo`) run an in-process fake Gmail with demo mail and two accounts.
