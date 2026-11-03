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
- **A failed submit is sent again.** When the invoke carrying actions fails
  (the core did not answer or rejected the batch), the UI keeps those actions
  ahead of newer ones and sends them again after 100 ms, doubling to at most
  5 s while it keeps failing. Rust drops numbers at or below its `ack`, so a
  batch that did arrive is not applied twice.
- **`Cmd+Q` waits at most 5 s** for Rust to commit the actions the page has
  sent (the composer's close among them), then quits anyway, so a core that
  stopped answering cannot keep the app from quitting. Rust then commits
  the UI jobs it has queued before it exits.

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

## Snooze, offline, notifications (step 6)

- Notification clicks: the notification plugin does not report clicks on
  macOS. When the window gains focus within 30 s of a notification shown while
  the app was in the background, the app treats it as a click: it switches to
  that account, opens Inbox, and highlights the thread.
- The Dock badge refreshes after every commit that can change unread counts.
- Wake from sleep is detected as a gap of more than three poll intervals.

## Look, matched to Superhuman screenshots

- The window is an off-white canvas (`surface`) with the content in a white
  rounded panel; the rail sits on the canvas. The rail is 56px rather than
  40px so its icon pill and account badges have room.
- In a conversation or composer the header is a back arrow and previous/next
  arrows overlaid on the panel, with the subject on the same row.
- The composer is a floating card with a soft shadow (the spec reserves
  shadows for the palette, sheets, and toasts). Recipients show as blue
  names, Cc/Bcc sit behind a chevron on the To row, and the footer has Send
  on the left and attach and discard icons on the right.

## Decided while designing the E2E suite

The full list is in `e2e/COVERAGE.md`, section 10. These depart from or add to
the spec's wording:

- **Split Inbox sheet.** Edits apply when the user presses Save, not at once,
  and the save is an undo entry (splits are undoable per §Undo).
- **Snooze date and time.** The "date-and-time choice" is a typed phrase in
  the snooze picker (for example "fri 9am").
- **Send with no recipients** is refused with an "Add a recipient" toast; the
  draft stays.
- **Held undo with no clear answer.** If `drafts.send` gets no clear answer
  and `drafts.get` then finds the draft, an undo held during the send wins:
  the draft is not sent again and returns as not sent.
- **Testing.** End-to-end tests drive the built UI in WebKit (Playwright)
  through the web bridge against the real core and the fake Gmail; Tauri's
  WebDriver has no macOS support. Unit tests are kept only for complex pure
  logic.

## Decided while fixing what the E2E suite found

- **Clicking a split name** in the list header moves the highlight to that
  split's first row, as `Tab` does, and scrolls its section header to the top.
- **"Your message was not sent" and "Send undone"** toasts with `Open draft`
  stay until clicked or closed, rather than Sonner's 4 s.
- **Sign-in.** Clicking "Add a Gmail account" (or the palette's `Add account`)
  while a sign-in waits in the browser starts over: the waiting one is
  cancelled and its loopback port closed.
- **A Gmail draft change skipped while a composer had it open** is read on
  close unless the user saved an edit after it (last writer wins).
- **`Shift+J` / `Shift+K` with nothing selected** extend from the highlighted
  row: it and the row moved to are both selected.
- **"Requests for the account on screen go first."** Request slots and the
  quota bucket are per account, so requests of different accounts never wait
  on each other. The scheduler still orders waiters by the account on screen,
  but that order has no effect anyone can observe.
- **A file still attaching** shows as a dashed chip. `Cmd+Enter` meanwhile
  waits for it and then sends; if it fails to attach, nothing is sent and the
  composer stays. Closing meanwhile takes the composer off screen at once and
  sends the close once the file is in the draft, so it is never discarded as
  untouched.
- **Edits made before a composer's first load** are kept and saved once the
  load is in: fields typed meanwhile win, files join the loaded ones, and a
  body typed before a resumed draft's body follows it. Send waits for the
  load; close leaves the screen at once and sends the close after it.
- **Keys typed after `r`, `f`, or `Enter`** before the reply editor has focus
  (its conversation still loading, or the composer not yet drawn) are the
  start of the reply, not shortcuts: printable keys and Backspace are kept
  and put in the editor when it opens. Leaving the conversation first drops
  them. `c` does not do this.
