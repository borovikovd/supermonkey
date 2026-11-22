# Test plan

The end-to-end cases, one line each. The test code holds each case's setup, steps, and expectations.

- **Format.** `- ID · priority · title`. Cases are grouped by spec file, and the id's prefix names the file. Each test title starts with its id, for example `test("TRI-03 # moves to Trash …")`. A new case takes the next free number in its group.
- **Priority.** P1: a core flow, a risk of losing or duplicating data, a security property, or a core state machine. P2: another rule, or a second partition of a P1 behavior. P3: a boundary that is costly to set up, or a detail close to cosmetic.
- **What runs.** Playwright on WebKit drives the built UI against the real Rust core through the web bridge, and the core talks to the in-process fake Gmail. The native shell is not exercised: a recording platform stands in for notifications, the Dock badge, opened URLs and files, and stored tokens.
- **Harness.** Seed scenarios are in `e2e/seeds.ts`, the page object and app time in `e2e/app.ts`, and the fake's control API (faults, holds, the request log) in `e2e/control.ts`. `e2e/harness.spec.ts` shows each piece in use.
- **The check.** `e2e/plan.spec.ts` checks that every case line parses, ids are unique and sit in their file's group, every case has a test in its file, every test id is a case, the lists below name only real cases and files, and every cell of the interruption grid names a case or a unit test, or says why the two can't meet.

## Cases

### accounts.spec.ts (ACC)

- ACC-01 · P1 · First run adds the first account, and its token goes to a file only the user can read
- ACC-02 · P1 · Add a second account from the palette, then switch with `Ctrl+1` and `Ctrl+2`
- ACC-03 · P1 · Switching closes overlays and returns each account to its own place
- ACC-04 · P1 · Switching with an open composer works like a second `Esc`
- ACC-05 · P2 · Avatar unread counts
- ACC-06 · P1 · Accounts off screen keep syncing, notifying, and sending
- ACC-07 · P1 · Remove an account
- ACC-08 · P1 · `invalid_grant` signs the account out; the queue and outbox wait, then recover
- ACC-09 · P2 · A failed sign-in adds nothing
- ACC-10 · P2 · Accounts with the same Gmail thread id stay separate
- ACC-11 · P1 · A task left from before Sign in again does not sign the account out again
- ACC-12 · P2 · Re-adding an account while its removal is still revoking keeps the new sign-in
- ACC-13 · P1 · Adding an account that is already signed in replays its queue once
- ACC-14 · P1 · An account with no stored token at launch shows Sign in again
- ACC-15 · P2 · Clicking the account on screen opens Settings; another account switches
- ACC-16 · P1 · A removal waiting on its revoke holds up no other account
- ACC-17 · P1 · A first sign-in shows the whole Inbox before reading the account's drafts
- ACC-18 · P1 · An account removed while its Sign in again waits in the browser comes back and syncs
- ACC-19 · P1 · A reply open when its account signs out stays on screen until it is closed
- ACC-20 · P1 · Removing the account on screen while its reply is open leaves no composer behind
- ACC-21 · P2 · Removing the account on screen shows the next account on the rail, or the one before the last
- ACC-22 · P1 · A sign-in left waiting while the user moves on finishes in the background, with what changed in Gmail meanwhile
- ACC-23 · P2 · Starting another sign-in cancels the waiting one, which shows no error
- ACC-24 · P2 · A sign-in cut off by a lost connection or a quit adds nothing, and the next one works
- ACC-25 · P1 · Adding an account again while a send, a discard, and an undo are on their way to Gmail does each once
- ACC-26 · P1 · A conversation and its reply still loading when the account signs out or is removed

### inbox-navigation.spec.ts (NAV)

- NAV-01 · P1 · Inbox rows show the sender, subject — snippet, time, and an unread mark
- NAV-02 · P1 · `j` and `k` move the highlight across split boundaries
- NAV-03 · P1 · `Enter` opens a conversation, `Esc` returns with the highlight; a click opens too
- NAV-04 · P1 · `j` and `k` inside a conversation open the next and previous conversation
- NAV-05 · P2 · `g` jumps and the rail
- NAV-06 · P2 · Empty states
- NAV-07 · P2 · The highlight moves when a poll removes the highlighted row
- NAV-08 · P2 · `j` keeps the prefetched neighbors it still needs
- NAV-10 · P1 · The highlight follows the mouse, so `e` acts on the row under it
- NAV-11 · P1 · A poll that empties the list clears the highlight, so `#` acts on the mail that arrives next
- NAV-12 · P1 · A row removed while another account is on screen moves that account's highlight
- NAV-13 · P1 · `g #`, `g !`, and a held `g` jump as typed on a keyboard, Shift and repeats included, and touch no mail
- NAV-14 · P2 · A list scrolled away from the highlight stays put while mail changes; moving the highlight, even with repeated `j` past the bottom edge, keeps it in view

### views.spec.ts (VIEW)

- VIEW-01 · P2 · A thread's latest message sets its row
- VIEW-02 · P1 · View membership comes from message labels
- VIEW-03 · P1 · Done lists every thread outside Inbox, from Gmail too
- VIEW-04 · P1 · Drafts lists every draft, and mail actions do nothing there
- VIEW-05 · P2 · Label, Trash, and Spam views also list from Gmail
- VIEW-06 · P1 · All Mail lists every thread outside Trash and Spam, from Gmail too

### conversation.spec.ts (CONV)

- CONV-01 · P1 · A long thread opens with read messages collapsed and folded, scrolled to the first unread
- CONV-02 · P2 · Fold boundary: four collapsed messages show, five fold
- CONV-03 · P2 · Expansion is fixed when the conversation opens
- CONV-04 · P2 · Expanding, collapsing, and selecting with the mouse
- CONV-05 · P2 · `n`, `p`, and `o` select and expand messages
- CONV-06 · P2 · `Shift+O` expands everything; pressing it again restores the default
- CONV-07 · P2 · Quoted history sits behind `•••`
- CONV-08 · P1 · Opening marks the thread read locally and on Gmail
- CONV-09 · P2 · A message that arrives while the conversation is open
- CONV-10 · P2 · If the selected message leaves, the latest is selected
- CONV-11 · P2 · Messages in trash or spam are hidden outside Trash and Spam
- CONV-12 · P2 · `Tab` cycles links, `Enter` opens one outside the app, `Esc` clears it
- CONV-13 · P2 · Shortcuts still work with focus inside a message body
- CONV-14 · P3 · Message iframes size to their content
- CONV-15 · P2 · A conversation left before it loaded is not marked read
- CONV-16 · P1 · The conversation that opens after an action is marked read
- CONV-17 · P2 · Message headers name people as Superhuman does, and the selected message is a raised card
- CONV-18 · P2 · The subject row's Done and Snooze buttons act like `e` and `h`
- CONV-19 · P1 · A message's dark-mode styles stay off when macOS is in dark mode
- CONV-20 · P2 · A conversation opened, closed, and opened again in one batch still reads its body
- CONV-21 · P1 · A message still downloading shows its snippet and a loading bar, then its body, growing in one step
- CONV-22 · P1 · A message labelled with a legacy charset but served as UTF-8 reads correctly
- CONV-23 · P2 · An address added to the account in Gmail is Me in a conversation already open
- CONV-24 · P2 · `r`, `Enter`, and `f` do nothing in a conversation that is gone
- CONV-25 · P1 · A reply open when its conversation turns gone closes as a second `Esc` would, keeping its text
- CONV-26 · P2 · A conversation still downloading follows Gmail: a reply joins it, and a deleted thread shows as gone
- CONV-27 · P2 · `z` while the conversation that opened after `e` still loads undoes the `e`, and that conversation opens and is read
- CONV-28 · P2 · A conversation still loading when the app quits stays unread

### triage-actions.spec.ts (TRI)

- TRI-01 · P1 · `e` on the list removes the row before Gmail answers, moves the highlight, and archives every message of the thread
- TRI-02 · P1 · `e` in an open conversation opens the next one, or the previous one, or returns to the empty list
- TRI-03 · P1 · `#` moves to Trash and never touches draft messages
- TRI-04 · P2 · `!` moves to Spam
- TRI-05 · P1 · Star and read toggles
- TRI-06 · P1 · `x` and `Shift+J` / `Shift+K` select; actions apply to the whole selection as one undo entry
- TRI-07 · P2 · `s` and `u` on a mixed selection follow the `l` rule
- TRI-08 · P1 · `Shift+E` returns to Inbox from any view, and the row leaves at once
- TRI-09 · P2 · `Esc` closes the open overlay before it clears a selection
- TRI-10 · P2 · Each action removes its row from the right lists
- TRI-11 · P1 · An action changes only the messages it recorded; a reply arriving meanwhile stays in Inbox
- TRI-12 · P1 · `e` marks the conversation read, and `z` makes the same messages unread again
- TRI-13 · P1 · `Cmd+A` selects from the highlight to the end of its split; `Cmd+Shift+A` the whole split
- TRI-14 · P1 · A selected row that a poll removes is not acted on
- TRI-15 · P1 · `s`, `u`, and `l` on Select all count every selected conversation, shown or not
- TRI-16 · P1 · `Shift+E` on mail in Trash or Spam reached by search takes it out of Trash or Spam
- TRI-17 · P1 · `j` on the last conversation and `k` on the first keep the selection

### undo.spec.ts (UND)

- UND-01 · P1 · `z` walks back through the stack
- UND-02 · P1 · Undo reverts only the labels the action changed
- UND-03 · P1 · Actions that make no entry are skipped by `z`
- UND-04 · P1 · Each account has its own undo history
- UND-05 · P2 · A quick `e` followed by `z` `z` undoes the right entries
- UND-06 · P2 · `z` still works after a restart
- UND-07 · P3 · Only the last 50 entries are kept
- UND-08 · P2 · Undoing a trash does not bring back a reply whose send was undone
- UND-09 · P1 · `z` while the action it undoes is on its way to Gmail, or queued offline, reaches Gmail after it

### labels.spec.ts (LAB)

- LAB-01 · P1 · `l` adds and removes a user label
- LAB-02 · P2 · `l` on a selection: remove only if every thread has the label
- LAB-03 · P2 · Create a label from the picker and apply it in one action
- LAB-04 · P1 · A label created offline is mapped to its Gmail id when the queue replays
- LAB-05 · P2 · If `labels.create` says the name exists, the existing label is used
- LAB-06 · P2 · `y` removes the current view's label, where it can
- LAB-07 · P2 · A label first seen in history is picked up
- LAB-08 · P2 · A label list read before a create landed does not delete the new label

### search.spec.ts (SRC)

- SRC-01 · P1 · `/` shows local results at once; focus moves to results; `Esc` restores the previous view
- SRC-02 · P2 · Local query language, and what is sent to Gmail
- SRC-03 · P1 · Gmail hits are merged in as their metadata arrives
- SRC-04 · P2 · The Gmail query waits for a 300 ms pause
- SRC-05 · P1 · Offline search shows cached mail and says the results are from this Mac
- SRC-06 · P2 · Actions on search results follow the removal table
- SRC-07 · P3 · Gmail results come 15 at a time, and each page is asked for once
- SRC-08 · P2 · A sent reply is found by local search
- SRC-09 · P2 · Search takes the header as in Superhuman, marks matches, and lists tips
- SRC-10 · P2 · A Gmail search stops searching when its account signs in again under it
- SRC-11 · P1 · Local search applies Gmail's operators before Gmail answers
- SRC-12 · P2 · A query local search can't evaluate waits for Gmail; offline it says the search needs Gmail
- SRC-13 · P2 · The next Gmail page loads near the oldest Gmail hit, not the list's end
- SRC-14 · P1 · A search or Gmail-listed view opened offline lists from Gmail when the network returns
- SRC-15 · P1 · An open search runs once Gmail can be reached again, though nothing else calls Gmail
- SRC-16 · P1 · A conversation opened from a search in Trash or Spam lists their messages, by `Enter` and `j`
- SRC-17 · P2 · Gmail hits whose threads are deleted or trashed while their metadata loads are not listed
- SRC-18 · P2 · A Gmail search whose account signs out or is removed while it lists stops searching, and the sign-in screen's `Enter` signs in

### splits.spec.ts (SPL)

- SPL-01 · P1 · Default routing to Important and Other, with counts in the header
- SPL-02 · P1 · `Tab` and `Shift+Tab` move between visible splits
- SPL-03 · P1 · Pin precedence: sender over domain over label
- SPL-04 · P1 · Pinning from the palette replaces an older pin; `z` restores it; a sender pin holds across syncs
- SPL-05 · P2 · Your own reply does not move a thread
- SPL-06 · P1 · Custom splits: add, order, rename, turn off, delete
- SPL-07 · P2 · Starter splits, and a flat inbox
- SPL-08 · P1 · Empty splits hide themselves, except after `g` `i` / `g` `o`
- SPL-09 · P3 · Counts cap at `999+`
- SPL-10 · P2 · Clicking a split name scrolls to its section
- SPL-11 · P1 · A rule the core refuses keeps the sheet open with every edit

### snooze.spec.ts (SNZ)

- SNZ-01 · P1 · The snooze picker at a normal hour
- SNZ-02 · P3 · "Later today" disappears at 18:00
- SNZ-03 · P1 · Snoozing takes the thread out of Inbox, here and in Gmail
- SNZ-04 · P1 · Snoozed threads wake on time, on launch, and after sleep, even offline
- SNZ-05 · P1 · `INBOX` added from outside clears a snooze; your own reply does not
- SNZ-06 · P1 · The app's own label changes, echoed by history, do not clear a newer snooze
- SNZ-07 · P1 · Undo, `Shift+E`, `e`, `#`, and `!` on snoozed threads
- SNZ-08 · P2 · `Enter` before the typed time's options arrive snoozes until that time

### compose-reply.spec.ts (REP)

- REP-01 · P1 · `r` on the list opens the reply composer under the thread, in view below a long message, ready to type
- REP-02 · P1 · What `r`, `Enter`, and `f` do on the list and in a conversation
- REP-03 · P1 · Reply and reply-all addressing
- REP-04 · P1 · A sent reply stays on the thread and carries Gmail's quote
- REP-05 · P1 · The reply follows the selected message
- REP-06 · P1 · Forward quotes the target, carries its attachments, and stays on the thread
- REP-07 · P1 · Which draft `r`, `Enter`, and `f` resume
- REP-08 · P1 · A half-written reply survives new replies
- REP-09 · P2 · Drafts from Gmail get a kind and a target
- REP-10 · P1 · Closing and sending the reply composer keep the conversation open
- REP-11 · P2 · A draft read from Gmail is split at its `gmail_quote`
- REP-12 · P2 · A Gmail draft's attachments are downloaded before its first upsert
- REP-13 · P1 · Reply keys pressed before the conversation loads open one composer
- REP-14 · P2 · Compose keys work while focus is inside a message
- REP-15 · P2 · Text typed before the reply opens goes into the reply
- REP-16 · P1 · A reply started while the message's body downloads quotes all of it
- REP-17 · P1 · A forward started while the message's body downloads carries all of it and its attachments
- REP-18 · P2 · A forwarded attachment that arrives with the body can be removed, and an untouched forward is still discarded
- REP-19 · P1 · A reply discarded before its target's body or its own first load arrives leaves nothing on Gmail
- REP-20 · P1 · `Cmd+Q` before a reply's first load arrives keeps what was typed

### compose-new.spec.ts (NEW)

- NEW-01 · P1 · `c` replaces the screen, and closing it returns to where it came from
- NEW-02 · P1 · Compose and send a formatted message, including the last keystrokes
- NEW-03 · P2 · TipTap shortcuts that clash with the compose table are replaced
- NEW-04 · P2 · Compose field shortcuts work whether the editor is focused or blurred
- NEW-05 · P1 · `Esc` in compose: the first blurs, the second closes; while blurred, only a few keys work
- NEW-06 · P1 · Shortcuts never fire while typing in compose
- NEW-07 · P1 · A fresh composer closed untouched leaves nothing behind
- NEW-08 · P2 · `Cmd+K` in compose
- NEW-09 · P1 · Sending with no recipients never loses the draft; text still in the To field is sent, even with the suggestion list open
- NEW-10 · P2 · From picker and the sending account
- NEW-11 · P1 · `Cmd+Shift+,` discards locally and in Gmail
- NEW-12 · P2 · Opening another composer, closing search, a new query, or another account's Split Inbox settings first closes the composer, as a second `Esc` would
- NEW-14 · P1 · An `Esc` pressed just after a composer opens, or after a key that moves focus in compose, counts, however late the focus would come

### recipients.spec.ts (RCP)

- RCP-01 · P1 · Autocomplete: at most six suggestions, keyboard choice, `Esc` stays in the field
- RCP-02 · P1 · Ranking and exclusions come from synced mail
- RCP-03 · P2 · Ways to turn typed text into a chip
- RCP-04 · P2 · `Backspace` and paste
- RCP-06 · P2 · `Tab` moves between fields unless the suggestion list is open
- RCP-07 · P2 · Cc and Bcc stay hidden until used
- RCP-08 · P2 · New mail updates the suggestions
- RCP-09 · P2 · Suggestions that answer after a chip is made stay closed
- RCP-10 · P2 · The autosave leaves an address being typed alone
- RCP-11 · P1 · Every address the core would refuse shows in `danger` and blocks Send until it is fixed
- RCP-12 · P2 · A chip removed while an address is being typed stays removed

### attachments.spec.ts (ATT)

- ATT-01 · P1 · Attach files and send them
- ATT-02 · P1 · Received attachments download only when opened
- ATT-03 · P1 · A forward made offline still sends its attachments
- ATT-04 · P2 · MIME tree shape and encoded headers
- ATT-05 · P1 · A send while a file is still attaching carries the file
- ATT-06 · P1 · Closing a fresh composer while a file is attaching keeps the draft
- ATT-07 · P1 · A downloaded attachment is quarantined before it opens
- ATT-08 · P2 · A file over 25 MB is refused before it is read; one of 25 MB is taken
- ATT-09 · P1 · `Cmd+Q` while a file is attaching quits once the composer's close is sent
- ATT-10 · P1 · A send waiting for a file still goes when the composer is closed meanwhile; a file that fails keeps the draft
- ATT-11 · P1 · A draft reopened while its close waits for a file keeps the new text and the file
- ATT-13 · P1 · A file Gmail serves inside the message opens and forwards
- ATT-14 · P1 · A file attached while a send waits for another goes with the send
- ATT-15 · P1 · Discarding while a send waits for a file cancels the send
- ATT-16 · P2 · The same file can be attached twice
- ATT-17 · P2 · An attachment that can't be opened says why
- ATT-18 · P1 · A file sent inline, as Apple Mail sends it, or an image the body doesn't show is an attachment, and a forward carries it
- ATT-22 · P2 · Files a discarded draft left behind when a quit cut off their clean-up go at the next launch; a kept draft's stay
- ATT-19 · P1 · An address typed but not yet a chip goes with a close or send that waits for a file
- ATT-20 · P1 · Removing the account while its composer's send or close waits for a file leaves nothing behind
- ATT-21 · P2 · A file attaching when the draft is deleted in Gmail joins the draft written again

### drafts.spec.ts (DRF)

- DRF-01 · P1 · Drafts autosave and update a single Gmail draft
- DRF-02 · P1 · `Enter` in Drafts opens a draft according to its kind
- DRF-03 · P1 · Edits made to a draft in Gmail, with the draft closed or open
- DRF-04 · P1 · A draft deleted or sent elsewhere
- DRF-05 · P1 · Discarding while offline is queued and replayed in order
- DRF-06 · P1 · A draft written offline reaches Gmail exactly once
- DRF-07 · P2 · Sending cancels a queued upsert
- DRF-08 · P2 · A webview reload or a crash with a composer open
- DRF-09 · P3 · The app's own drafts are read back after the account is re-added
- DRF-10 · P1 · Text deleted while its save is on the way stays deleted
- DRF-11 · P1 · Typing before a reply's first load keeps its filled fields
- DRF-12 · P1 · An edit saved while the draft's upsert is in flight still reaches Gmail
- DRF-13 · P1 · A draft discarded while its first upsert is in flight stays gone
- DRF-14 · P1 · A second Gmail draft carrying a local draft's id is deleted as an extra copy
- DRF-15 · P1 · A draft reconcile on a stale list keeps a draft written meanwhile
- DRF-16 · P1 · A Gmail edit read back after the draft was reopened and edited does not replace the edit
- DRF-18 · P1 · A crash after `drafts.create` reached Gmail, before its ids were stored, leaves one draft
- DRF-19 · P1 · An edit still waiting to reach Gmail is not replaced by an older Gmail version
- DRF-20 · P1 · `j` and `k` in a conversation opened from Drafts open the other drafts' threads and keep the Drafts highlight
- DRF-21 · P1 · A draft deleted on Gmail while an edit waits to reach Gmail is written again with the edit
- DRF-22 · P2 · A draft deleted elsewhere just before its composer loads leaves the screen and says so
- DRF-24 · P2 · A reply emptied again after it reached Gmail is emptied in Gmail too
- DRF-23 · P1 · A draft discarded after a create with no answer stays gone across a restart, even when Gmail failed to read its copy

### undo-send.spec.ts (SND)

- SND-01 · P1 · The send waits ten seconds, then goes out once
- SND-02 · P1 · Undo while waiting cancels the send and reopens the composer
- SND-03 · P1 · `z` reaches a send only while it is the newest entry; the toast's Undo always does
- SND-04 · P1 · Undo after the send was accepted trashes only that message
- SND-05 · P1 · Undo while sending is held until the send resolves
- SND-06 · P1 · A rejected send brings the draft back
- SND-07 · P1 · Offline at send time: "Email delayed", then sent once online
- SND-08 · P1 · `drafts.send` with no clear answer never sends twice
- SND-09 · P1 · Outbox rows across a quit and relaunch
- SND-10 · P2 · Send toasts stay across accounts
- SND-11 · P2 · Undoing a waiting send while another composer is open
- SND-13 · P1 · A draft reconcile in the undo window keeps the version that was sent
- SND-14 · P1 · A crash while the send writes the draft still sends the final version
- SND-15 · P1 · A held undo resolves at once when the send could not reach Gmail
- SND-16 · P1 · A send toast closed before the send went out takes its undo entry with it once sent
- SND-17 · P1 · An Undo that reaches the core only after the send went out opens no composer
- SND-18 · P1 · A send while the draft's first upsert is in flight goes out once and leaves no draft
- SND-19 · P1 · A send the core refuses says it was not sent and brings the draft back
- SND-20 · P2 · An Undo the core refuses says why and opens no composer
- SND-21 · P1 · A held undo resolves at once when the send's access token could not be refreshed
- SND-22 · P1 · A sent reply stays on the thread, marked Sent, until its message lands without waiting for a poll
- SND-23 · P1 · A send settled as sent with no message to show leaves the thread once its toast closes
- SND-24 · P1 · A send whose account signs out or is removed while `drafts.send` is on its way
- SND-25 · P1 · An Undo while the draft's first upsert is in flight reopens the composer and leaves one Gmail draft
- SND-26 · P2 · A reply key on a thread whose reply is waiting or sending starts a new draft and leaves the send alone
- SND-27 · P1 · A send whose Gmail draft is edited or deleted elsewhere in the undo window sends what was sent here, once
- SND-28 · P2 · Undoing a waiting send while another composer's first load is on its way keeps what was typed there

### offline.spec.ts (OFF)

- OFF-01 · P1 · Airplane mode: read, search, autocomplete, and act; it all lands when the network returns
- OFF-02 · P1 · Every kind of queued action replays, mail changes and draft writes each in order
- OFF-03 · P1 · The queue survives a restart
- OFF-04 · P1 · A queued action wins on the messages it recorded, without hiding newer mail
- OFF-07 · P1 · A Gmail answer read before an action landed does not undo it
- OFF-05 · P1 · A rejection during replay rolls back that one action, and replay continues
- OFF-06 · P2 · Losing the network during replay leaves the rest queued
- OFF-08 · P2 · Conversations opened offline read only the bodies still on screen when the network returns
- OFF-09 · P1 · A poll during the replay of a queued action keeps the action applied
- OFF-10 · P1 · A draft write on its way to Gmail does not hold up an archive made after it

### gmail-errors.spec.ts (ERR)

- ERR-01 · P1 · Transient failures retry silently and keep the row as the user left it
- ERR-02 · P2 · Back-off timing and `Retry-After`
- ERR-03 · P2 · A quiet sync status after 30 s of retrying
- ERR-04 · P1 · A rejection rolls the action back and shows Gmail's error
- ERR-05 · P1 · An expired token is refreshed once, and the request is retried
- ERR-06 · P1 · A 404 on part of an action drops only the missing messages
- ERR-07 · P1 · `drafts.create` with no clear answer never makes a duplicate draft
- ERR-08 · P2 · A 404 while fetching a body is dropped silently
- ERR-09 · P3 · A rejected undo or label create
- ERR-10 · P2 · The quiet sync status clears once Gmail answers after a relaunch

### sync.spec.ts (SYN)

- SYN-01 · P1 · The connect sync matches Gmail
- SYN-02 · P1 · Mail that changed while the app was closed shows after relaunch
- SYN-03 · P1 · Polls bring in changes made elsewhere
- SYN-04 · P1 · History too old: a full resync that sends no notifications
- SYN-05 · P3 · The full resync keeps a thread that moved during the listing
- SYN-06 · P2 · `sendAs` and labels refresh on focus
- SYN-07 · P3 · An opened thread fetches its bodies right away
- SYN-08 · P1 · A slower thread fetch never undoes a newer one
- SYN-09 · P2 · A connect sync retried after a failure misses no change made in between
- SYN-12 · P1 · A quit during the first sync loses none of it: starred and recent sent mail are read after the relaunch
- SYN-13 · P3 · A quit during the contacts backfill does not end it: it runs again after the relaunch
- SYN-11 · P2 · A slower label or `sendAs` read never undoes a newer one

### notifications.spec.ts (NOT)

- NOT-01 · P1 · New mail in either account raises a notification and adds to the Dock badge
- NOT-02 · P1 · When a notification is not raised
- NOT-03 · P1 · More than three new messages in one poll become one summary
- NOT-04 · P1 · A muted account does not notify and is left out of the badge
- NOT-05 · P1 · The Dock badge counts unread Inbox threads
- NOT-06 · P2 · Clicking a notification opens that account's Inbox on the thread

### settings.spec.ts (SET)

- SET-01 · P2 · What the Settings sheet holds, and how it opens
- SET-02 · P1 · Remote images
- SET-03 · P2 · Hint text is 11px, the shortcut chips are readable, and the shortcut sheet's rows never overlap
- SET-05 · P1 · With remote images off, nothing remote loads in a message or a reply's quote
- SET-06 · P2 · `Esc` closes a sheet from one of its fields, and a sheet open over a composer

### palette.spec.ts (PAL)

- PAL-01 · P1 · Open, filter, run, and close the palette
- PAL-02 · P2 · Palette commands do what their keys do
- PAL-03 · P1 · The palette lists only the commands that work where it opened; the shortcut sheet lists every shortcut
- PAL-04 · P2 · An empty Inbox offers no mail actions and hints only the palette

### security.spec.ts (SEC)

- SEC-01 · P1 · Hostile HTML cannot run script or navigate
- SEC-02 · P1 · Mail text never becomes markup outside the frame
- SEC-03 · P1 · Links never navigate the app window
- SEC-04 · P2 · The webview holds no Gmail credentials
- SEC-06 · P1 · The UI runs under the release app's content security policy
- SEC-07 · P1 · Links in a reply's quoted text open in the browser, never in the composer
- SEC-08 · P1 · Hostile HTML in a reply's quoted text cannot run script or navigate

### ui-consistency.spec.ts (UIC)

- UIC-01 · P1 · Rows you just acted on never flash back during fast triage or a poll
- UIC-02 · P2 · A reload keeps queued actions and keeps action numbers going
- UIC-03 · P2 · An action whose submit fails is sent again
- UIC-04 · P2 · A row that changed while no list held it is not shown from an old copy
- UIC-05 · P1 · `Cmd+Q` quits only once the composer's close has committed
- UIC-06 · P1 · An action the core rejects tells the user why
- UIC-07 · P1 · An action the core can't read is refused, and the actions after it still apply
- UIC-08 · P1 · An action stays applied when the one before it is acked

### unwanted.spec.ts (UNW)

- UNW-01 · P1 · `Cmd+U` unsubscribes with one click when the list offers it
- UNW-02 · P1 · An email unsubscribes, and "mark done all" archives the sender's Inbox mail
- UNW-03 · P1 · A list with only a web page opens it, and "trash all" trashes the sender's mail
- UNW-04 · P2 · No unsubscribe link, or no network, says so; the archive still happens
- UNW-05 · P1 · Block makes a Gmail filter to Trash, trashes the conversation, and keeps new mail out
- UNW-06 · P2 · Blocked senders lists the blocks, and Unblock deletes the filter
- UNW-07 · P1 · An account signed in before filters were asked for is offered Sign in again
- UNW-08 · P1 · Mark spam, and block the domain
- UNW-09 · P1 · Unsubscribe, block, and unblock act only on the account's own thread and block filter
- UNW-10 · P1 · Block and unsubscribe act on their own account when another one is on screen by then

## Interruptions

Each operation that takes time × each event that can arrive mid-way (AGENTS.md, Concurrency). A cell names the cases that force the interleaving, the operation provably still in progress when the event comes, or says why the two can't meet.

Events: close, repeat, send, discard, undo, quit, elsewhere, account, offline.

### Conversation load

- close: CONV-15, OFF-08
- repeat: CONV-20, REP-13
- send: n/a: `Cmd+Enter` acts only in a composer; a composer over a loading conversation is the Composer load row
- discard: n/a: `Cmd+Shift+,` acts only in a composer; a composer over a loading conversation is the Composer load row
- undo: CONV-27
- quit: CONV-28
- elsewhere: CONV-26
- account: ACC-26
- offline: OFF-08

### Composer load

- close: DRF-11, REP-13
- repeat: REP-13
- send: REP-16, ATT-03
- discard: REP-19
- undo: SND-28
- quit: REP-20
- elsewhere: DRF-22
- account: ACC-26
- offline: REP-12, ATT-03

### Draft save

- close: DRF-19, DRF-21, ERR-07
- repeat: DRF-12, DRF-10, DRF-06
- send: SND-18, DRF-07, SND-13
- discard: DRF-13, DRF-05
- undo: SND-25
- quit: UIC-05, DRF-18
- elsewhere: DRF-19, DRF-21, DRF-13
- account: ACC-19
- offline: DRF-05, DRF-06, DRF-07, DRF-19, ERR-07

### Attach

- close: ATT-06, ATT-10, ATT-19
- repeat: ATT-11, ATT-14
- send: ATT-05, ATT-14, ATT-19
- discard: ATT-15
- undo: n/a: adding a file makes no undo entry, and another send's Undo closes this composer as a second `Esc` would, which is the close cell
- quit: ATT-09
- elsewhere: ATT-21
- account: ATT-20
- offline: n/a: attaching copies the file locally and calls no Gmail; a send waiting for it then waits offline like any send (SND-07)

### Send wait

- close: ACC-06, SND-10, SND-06, SND-16, SND-02
- repeat: SND-26
- send: n/a: the draft leaves the screen and Drafts when sent and is never resumed, so it can't be sent again
- discard: n/a: no composer or Drafts row reaches a draft in the outbox
- undo: SND-02, SND-03, SND-10, SND-11, SND-07, SND-17
- quit: SND-09
- elsewhere: SND-27
- account: ACC-07, ACC-08
- offline: SND-07, OFF-01, SND-09

### Send

- close: SND-23
- repeat: SND-26
- send: n/a: the draft is in the outbox and is never resumed
- discard: n/a: no composer or Drafts row reaches a draft in the outbox
- undo: SND-05, SND-15, SND-21, SND-17, `crates/sm-core/src/outbox/machine.rs`
- quit: SND-09, SND-14
- elsewhere: SND-23
- account: SND-24
- offline: SND-08, SND-07, SND-21

### Action

- close: ACC-06, LAB-01, LAB-03
- repeat: UIC-01, OFF-06
- send: OFF-01
- discard: OFF-02
- undo: UND-09
- quit: OFF-03
- elsewhere: TRI-11, UIC-01, OFF-04
- account: ACC-07, ACC-08, ACC-13
- offline: OFF-06, OFF-01, OFF-02, ACC-08

### Search listing

- close: SRC-14
- repeat: SRC-04, SRC-14, SRC-07
- send: n/a: a listing only merges hits into its own subscription; a send never reads or changes one
- discard: n/a: a listing only merges hits into its own subscription; Drafts is not a Gmail-listed view
- undo: n/a: list membership comes only from Rust's patches, whatever a listing is doing
- quit: n/a: a listing belongs to an open subscription and is never stored; a relaunch opens no search
- elsewhere: SRC-17
- account: SRC-18, SRC-10
- offline: SRC-15, SRC-14

### Sign-in

- close: ACC-22
- repeat: ACC-23, ACC-09
- send: ACC-25
- discard: ACC-25
- undo: ACC-25
- quit: ACC-24
- elsewhere: ACC-22
- account: ACC-18, ACC-11
- offline: ACC-24

## Not covered end-to-end, and why

- Window behavior (a second launch focuses the window, closing hides it, a Dock click shows it, size and position restored but not visibility, the window shown only once the page connects or after 5 s, the overlay title bar) and quitting from the menu bar or macOS: the native shell is not exercised. The suite's quit is the E2E server asking the page itself (UIC-05, ATT-09); `tests/menu.rs` checks that the menu bar's Quit is the app's own. Checked by hand on a real Mac with the release build, on a test account:
  - type in a composer and `Cmd+Q` within 0.5 s: the text is there on relaunch, and in the Gmail draft;
  - the same with the window hidden (`Cmd+W`) and with it minimized;
  - archive, `Cmd+Q` at once: archived on relaunch, and in Gmail once online;
  - archive, then Quit in the Dock at once: archived on relaunch;
  - in a debug build, with the page paused at a breakpoint in the Web Inspector: the app quits 10 s after `Cmd+Q`.
- Real Google OAuth (the system browser, the unverified-app warning, a production consent screen): the fake OAuth stands in, and ACC-01 checks the token file and the auth URL's scope, PKCE, and loopback. Checked by hand against two real accounts.
- Performance budgets, the keydown-to-paint meter, 60 fps scrolling, cold launch: CI WebKit is not the M1 target. Checked on the perf fixture (`crates/sm-core/src/tests/perf.rs`).
- Look, Motion, and Reduce Motion: visual, not behavior; SET-03 checks the hint size and the shortcut sheet. Design review.
- The quota bucket, the six priorities, and at most 8 requests per account: internal scheduling, not stable through the UI; SYN-07 is a smoke check. `crates/sm-gmail/tests/gmail/scheduler.rs`.
- The back-off jitter: random; ERR-02 checks `Retry-After` and that gaps grow. `crates/sm-gmail/tests/gmail/scheduler.rs`.
- OAuth protocol edges (a wrong `state`, concurrent 401s sharing one refresh, refresh before expiry, secrets out of debug output): below the UI. `crates/sm-gmail/tests/gmail/oauth.rs`.
- How each HTTP response maps to an outcome: the ERR cases and SND-08 cover each outcome's behavior; the mapping is implied.
- The sanitizer policy, element by element: SEC-01 checks a hostile sample end to end. `crates/sm-mime/tests/sanitize.rs`.
- MIME parsing robustness (charsets, malformed headers, random input): fuzzing, not E2E; ATT-04 has one encoded-subject round trip. `crates/sm-mime/tests/no_panic.rs`.
- The query language's grammar, errors, FTS translation, and Unicode matching: pure parsing; SRC-02, SRC-11, SRC-12, and SPL-06 check one query per family and the wiring. `crates/sm-domain/src/query/tests.rs`.
- Snooze times across weekdays and DST, and the phrase grammar: pure; SNZ-01 checks a normal hour and SNZ-02 the 18:00 boundary. `crates/sm-domain/src/snooze/options.rs`.
- Contact scores (recency halving every 90 days, word-start matching): pure; RCP-02 checks tier order and exclusions from real mail. `crates/sm-domain/src/contacts.rs`.
- Person-to-person routing over every label combination: pure; SPL-01 checks the list flag and four categories. `crates/sm-domain/src/tests.rs`.
- Writer and channel ordering (`rev`, `ack`, patches against first loads): races E2E can't force; UIC-01 checks the visible no-flash rule. `crates/sm-core/src/tests/writer_channel.rs`.
- The mirror's re-application of pending actions in detail: UND-05, UIC-01, TRI-01, and TRI-10 cover the behavior; exact `ack` interleavings can't be forced E2E.
- Table schemas and `rev` and action-number values: internal.
- A due snooze not yet woken staying in Snoozed: true only while the app is not running; SNZ-04 checks that Gmail keeps it out of Inbox meanwhile and that it wakes before the first paint.
- The full resync's step-3 race at scale, billing and daily units: cost and scale facts.
- Real sleep and wake: simulated by `app.sleep` and `app.sleepAndWake`.
- Label colors, nested-label editing, permanent delete: out of scope.

## Kept unit and property tests

Unit tests only for complex algorithms, security properties, and races the suite can't reach (AGENTS.md); the suite doesn't repeat their partitions. Paths are under `src-tauri/`.

- `crates/sm-domain/src/query/tests.rs`: lenient and strict parsing and its errors, operator case and precedence, search operators and scope, split rules refusing search terms, FTS5 translation and escaping, `is:list` stripped for Gmail (E2E: SRC-02, SRC-11, SRC-12, SPL-06).
- `crates/sm-domain/src/tests.rs`: a thread's latest message, digest and views, label changes and their undo, split routing and pin precedence, the latest message from someone else, and which message a Gmail draft answers (SPL-03, SPL-07).
- `crates/sm-domain/src/snooze/options.rs` and `crates/sm-domain/src/snooze/text.rs`: options before and after 18:00 and on every weekday, DST gaps and folds, the phrase grammar (SNZ-01, SNZ-02).
- `crates/sm-domain/src/contacts.rs`: autocomplete matching, ranking, and no-reply addresses.
- `crates/sm-mime/tests/sanitize.rs`: the hostile fixture, what is kept and dropped, `rel=noopener`, `cid:` to `data:`, text extraction (SEC-01).
- `crates/sm-mime/tests/payload.rs`: a charset with no decoder read as undeclared text; the attachments inside an attached message, and not its body or inline images; an image is inline only when the HTML body shows it, by a plain or percent-encoded `cid:` reference, here and inside an attached message, and only those are fetched for the body (ATT-18).
- `crates/sm-mime/tests/no_panic.rs`: 3,000 seeded random inputs through every parser and the sanitizer.
- `crates/sm-mime/tests/addresses.rs`: `is_valid_email` against the address table the composer's copy of the rule is held to (RCP-11).
- `crates/sm-mime/src/unsubscribe.rs`: the `List-Unsubscribe` parser and when one-click applies.
- `crates/sm-mime/src/outgoing.rs`: user text can't add headers to an outgoing message.
- `crates/sm-gmail/tests/gmail/scheduler.rs`: priorities, concurrency, quota, and a removed account starting afresh.
- `crates/sm-gmail/tests/gmail/oauth.rs`: the `state` check, one refresh shared by concurrent 401s, secrets out of debug output.
- `crates/sm-gmail/tests/gmail/client.rs`: one-click unsubscribe posts only to `https` and follows no redirect.
- `crates/sm-fake-gmail/tests/holds.rs`: a release racing a held reply never strands it.
- `crates/sm-store/tests/store/writer.rs`, `crates/sm-store/src/writer.rs`, and `crates/sm-store/src/reply.rs`: writer priority and order, rollbacks and savepoints, panics, `rev`, stopping, awaiting results.
- `crates/sm-store/src/open.rs`: a database of another schema version, or not a database, is moved aside and a fresh one made; a locked one, or one that can't be opened, is left in place and the open fails.
- `crates/sm-core/src/tests/writer_channel.rs`: proptest that a list built from its first load and patches equals a fresh load at that `rev`; action numbers after a reload, first loads after a failed commit, `ack` never moving back and a failed batch's actions refused no later than the message that acks them, a panicking commit still going idle, searches and label reads whose commit failed (UIC-01).
- `crates/sm-core/src/subs/diff.rs`: list diffs as moves.
- `crates/sm-core/src/tests/clock.rs`: the activity tracking `settle()` relies on, and a stopping task group waiting for the groups inside it.
- `crates/sm-core/src/outbox/machine.rs`: every send event in every outbox state.
- `crates/sm-core/src/drafts/lifecycle/state.rs`: every draft transition in every stage: the untouched rule, waits for parts and a target's body, Gmail's ids and a create in question, a skipped Gmail change.
- `crates/sm-core/src/tests/perf.rs` (ignored; needs `SUPERMONKEY_PERF_DB`): local search and autocomplete against the performance budgets.
- `crates/sm-bridge/src/web.rs`: the debug web bridge answers only loopback hosts and its own origins.
- `tests/layers.rs`: the crate layering: which workspace crates each may depend on, and which crates alone may use `rusqlite`, `reqwest`, `tauri`, and `axum`.
- `tests/menu.rs`: the menu bar is Tauri's standard one with the app's own Quit in place of the predefined one, which would end the app without asking the page (UIC-05).
