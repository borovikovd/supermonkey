# Deviations from SPEC.md

Choices made where the spec was silent, ambiguous, or could not be built as
written. Each keeps the spec's intent.

## Build and tooling

- **Workspace crates.** The Rust side is split into crates under
  `src-tauri/crates/` by layer (`ub-domain`, `ub-mime`, `ub-gmail`,
  `ub-store`, `ub-ipc`, `ub-core`, `ub-bridge`, `ub-testkit`, and
  `ub-fake-gmail`, the fake Gmail for tests); ARCHITECTURE.md, section 3,
  says what each holds.
- **Debug web bridge.** Debug builds started with `UBERMENSCH_WEB_BRIDGE=1`
  serve the same command surface on `127.0.0.1:1430`, so the UI can run in a
  browser against the real Rust core for automated checks. Release builds
  never start it.
- **Perf measurements** were taken on this Mac in Chrome through the web
  bridge, not on an M1 MacBook Air in WKWebView.
- The local search and autocomplete bench (`ub-core`'s ignored `perf` tests)
  times the core's part of a keystroke on a copy of the perf fixture that the
  system already has cached, as it has a running app's database. Reading a
  cold disk is part of the cold-launch budget, not a keystroke's.

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
- A draft reconcile (`drafts.list`, then reads) leaves alone a local draft
  that changed after its first request, or is being written to Gmail, as the
  full resync does for threads: Gmail's answer may predate it. The next
  reconcile, which the write itself triggers through history, looks again. A
  Gmail change read for a draft whose composer opened meanwhile is read when
  that composer closes, as if the composer had been open all along.
- Notification clicks: the macOS notification plugin does not report clicks,
  so a click only brings the app forward. See Notifications (step 6).

## Compose (step 3)

- Attachments are picked with the webview's file input and copied into the
  draft's folder by an `add_attachment` command, so no dialog plugin is added.
- One `draft_upsert` op per draft at a time; it writes the draft's latest
  state when it runs, so fast typing does not queue a write per save. A save
  that lands while that op is running queues it again, so the write that
  follows carries the save.
- Undo of a waiting send clears the draft's defaults, so the reopened draft is
  never discarded as untouched.
- A Gmail draft found by its `X-Ubermensch-Draft` header (Gmail errors): when
  the local draft already has a live Gmail draft, the one found by header is
  the extra copy and is deleted. Otherwise only its Gmail ids are stored on
  the local draft; the local text is kept, since the copy holds text this app
  wrote and the local draft is never older.
- Demo mode: debug builds with `--features fake` and `UBERMENSCH_FAKE=1`
  (`vp run demo`) run an in-process fake Gmail with demo mail and two accounts,
  on a data folder in the system's temporary directory emptied at each launch.

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
- In a conversation or composer the header is a round back button and a
  round previous/next pill, white with a soft shadow, at the panel's top
  left, with the subject on the same row. A narrow window moves the column
  right so the buttons never cover the subject.
- The conversation subject is 24/32 at regular weight (the spec says 18/27
  at 600), as in Superhuman. Mark Done and Snooze icons sit at the right of
  the subject row and do what `e` and `h` do.
- The selected message is a white card with a soft shadow and a light
  accent edge (`accent-edge`, the accent at half strength); the others sit
  flat on the panel with no dividers. Every message spans the same width,
  so selecting one never moves its text. The reply composer card lines up
  with the message cards.
- Message headers read like Superhuman's: "Carol Chen to Dave, Erin & Me".
  Recipients go by first name, or by the address's local part when there
  is no name. The account's own addresses, aliases included, are "Me", put
  last, and mail to this account alone names only the sender. The time is
  the list's short form; hovering shows the full date.
- The composer is a floating card with a soft shadow (the spec reserves
  shadows for the palette, sheets, and toasts). Recipients show as blue
  names, Cc/Bcc sit behind a chevron on the To row, and the footer has Send
  on the left and attach and discard icons on the right.
- **App icon.** The spec names none. `src-tauri/icons/app-icon.svg` is the
  source; `vp run tauri icon src-tauri/icons/app-icon.svg` makes every size
  (delete the `android` and `ios` folders it adds). It is a dark violet tile,
  as Superhuman's is, holding one faceted mark: Zarathustra's mountain, a
  peak lit on one face, with the sun rising in its notch and a low dawn
  glow behind.
- **`e` marks read, and All Mail exists, as in Superhuman.** Mark Done also
  removes `UNREAD` (Superhuman's docs are silent on it; this follows how the
  app behaves), and `z` restores it only where it was. All Mail is `g a` and
  the palette's "Go to All Mail", with no rail icon, as in Superhuman's
  shortcut sheet.
- **Unsubscribe** reads the unsubscribe headers when `Cmd+U` runs rather than
  storing them, so no migration was needed. It needs the network and is not
  queued: a queued unsubscribe could fire long after the user stopped caring.
  Debug builds also allow a one-click `POST` to loopback `http`, where the E2E
  server stands in for a list.
- **Block is a Gmail filter**, so it works with the app closed and on other
  devices. It needs `gmail.settings.basic`, so accounts signed in before it
  sign in again once, when they first block. The filter trashes the mail (as
  Gmail's "Delete it"), and blocking also trashes the conversation it was
  done from, since the filter only acts on mail that arrives later.
- **Search looks like Superhuman's**: the query in the header in line with
  the senders, marked matches in results, and a Tips column of operators.
  Superhuman's Tips list includes Gmail-only operators; ours does too, and
  those work through Gmail's results, so offline they find nothing.
- In Drafts the hint bar reads `Enter to open draft · C to compose · ⌘K for
  commands`: the spec's example hints (`E`, `H`) do nothing there.

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
- **A highlighted row that leaves with no neighbor known** (the list emptied,
  or the place it sat was never seen) leaves no highlight, so the list
  highlights its top row once it has one, as on a first visit. Each account
  remembers its own place, so a row removed while another account is on
  screen moves the highlight as it would on screen.
- **A selected row that leaves without the user acting on it** is no longer
  acted on. On the list, actions apply only to rows still shown; with none
  of the selection shown, they apply to the highlight.
- **"Requests for the account on screen go first."** Request slots and the
  quota bucket are per account, so requests of different accounts never wait
  on each other, and there is no ordering by the account on screen: it
  would have no effect anyone could observe.
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

## Decided in the rewrite: mail actions and the op queue

- **Rejection toasts** read "Gmail rejected a change (status): Gmail's
  message". Two answers that are not a plain 4xx count as rejections too, so
  the change rolls back instead of retrying forever: a `batchModify` 404
  where every message still exists (a label it names is gone), and a
  `labels.create` 409 for a name `labels.list` then does not show.
- **One op at a time per account, across worker replacement.** An op's
  Gmail call and the write of its result run outside the worker that
  started them, and the next worker's runner waits for them, so a second
  sign-in never sends a queued op twice (ACC-13). Quitting can still cut an
  op off; it stays queued and is sent again on the next launch, which
  `batchModify` and `labels.create` (by its 409) make harmless.
- **A label list read before a label made here got its Gmail id** keeps
  every label made here, not only that one; the next refresh drops any that
  Gmail no longer has.
- **A label made here that merges into one Gmail already had** (a refresh
  stored Gmail's copy before the create answered) is renamed in queued ops
  and undo entries too, so they keep naming a label that exists.

## Decided in the rewrite: compose

- **A draft's files are read only from the app's own folders.** The paths
  in a save come from the webview, which is not trusted: a file added to a
  draft is read only from that draft's folder in the app's data folder, and
  an attachment a draft carries only from the account's downloads (else it
  is downloaded again). A file that is gone, or outside those folders,
  rejects the write with a toast instead of sending without it.
- **Files added to a draft** are kept as `drafts/<draft>/<n>/<name>` in the
  app's data folder, with the id `file-<n>`, so the file keeps the name the
  user picked and two files added at once never share one. The folder goes
  when the draft is sent, discarded, or its account removed.
- **A poll that sees a draft change** reconciles the drafts only. The
  connect sync and the full resync also queue each Gmail draft's thread for
  metadata and bodies, as SPEC Sync lists them.
- **A file added to a draft is at most 25 MB**, Gmail's own limit for
  attachments; SPEC sets none. A larger one is refused with a toast before
  its content is decoded or written.
- **A reply or forward started before its target's body is stored** quotes
  the target's snippet meanwhile, so the composer shows what is known. When
  the body lands, Rust fills the draft: the whole quote, and for a forward
  the target's attachments, which the composer then lists. Those count as
  the draft's defaults, so an untouched forward is still discarded, and a
  save the composer made before it saw them does not remove them; one the
  person removes stays removed. Such a draft is never written to Gmail with
  the snippet's quote: its upsert or send reads the target's body first, at
  priority 1, so a send made before the body arrived waits for it (offline,
  as any send waits). If Gmail no longer has the target, the snippet's
  quote goes. SPEC says only that Rust builds the quote from the stored
  target. A draft read from Gmail keeps Gmail's quote.

## Decided in the review of the rewrite

- **Files from mail are quarantined.** Every attachment the app writes to
  disk gets `com.apple.quarantine` (flags `0081`, the time, `Ubermensch`,
  an event UUID) before it can be opened, so Gatekeeper checks it as it
  checks a browser download. SPEC Reading says only "open with the default
  app".
- **A sign-in redirect with another `state`** is answered 400 and ignored;
  the sign-in keeps waiting for its own. A page that finds the loopback
  port can no longer end the sign-in the user is in the middle of.
- **Unblock deletes only a block.** The filter id comes from the webview:
  the core lists the account's filters and deletes it only if it is one of
  the blocks Blocked senders shows.
- **An account removed while its Sign in again waits in the browser** comes
  back when the person finishes that sign-in, under a new id, as Add
  account would bring it back. The sign-in finished after the removal, so
  it is the person's last word; SPEC does not say.

## Decided when local search learned Gmail's operators

- **A query local search can't evaluate lists only Gmail's hits.** SPEC
  said local search ignores operators it does not support. Ignoring one
  made it match everything, so `in:sent venue` listed incoming mail and the
  merged list was a superset of Gmail's. Now local search evaluates the
  operators the Tips column lists and their siblings, and for any other
  (`filename:`, `larger:`, `from:(a b)`, a date it can't read) it lists
  nothing: the list says `Searching Gmail…`, and offline that the search
  needs Gmail. The core sends `needsGmail` with the search's list, so the UI
  does not parse the query itself.
- **Search operators evaluate per message, as Gmail's `threads.list` does.**
  A conversation matches when one message matches the whole query, so
  `-in:inbox` also finds an Inbox conversation holding your own reply, as
  Gmail does. User labels stay the conversation's, as for `label:`.
- **Dates are local.** `after:2026/01/31` is local midnight in this Mac's
  time zone, as Gmail's web search reads it (the Gmail API documents
  Pacific time). `after:` includes that instant, `before:` excludes it.
  `newer_than:` and `older_than:` count back calendar days, months, and
  years, so a DST change does not shift them by an hour.
- **`in:snoozed` is this app's snooze.** This app snoozes by removing
  `INBOX` and storing the time locally, which Gmail does not know; Gmail's
  own `in:snoozed` hits are merged in as usual.
- **`in:trash`, `in:spam`, `in:anywhere`, and `in:drafts` widen what local
  search looks at** whenever the query names them, even negated, as Gmail
  does. Other queries still skip drafts and mail in Trash and Spam.
- **`from:me` is this account's addresses in search only.** A split rule
  keeps reading `me` as text, as it always has, so no saved rule changes.
- **Split rules keep their language.** Negation, `in:`, `is:` (other than
  `is:list`), `has:`, `category:`, and dates are refused in a rule, as they
  were. Read state, stars, and dates change without the conversation
  changing, so a rule on them would leave conversations in a stale split;
  `in:` adds nothing to rules that only see Inbox mail. Negation would make
  sense in a rule but is left for when rules need it.
