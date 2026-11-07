# Test plan

The end-to-end cases, one line each. [SPEC.md](../SPEC.md) is the source of truth: a case asserts what the spec says, and a change in behavior starts in the spec. The test code holds each case's setup, steps, and expectations.

- **Format.** `- ID · priority · title · §SPEC section, …`. Cases are grouped by spec file, and the id's prefix names the file. Each test title starts with its id, for example `test("TRI-03 # moves to Trash …")`. A new case takes the next free number in its group.
- **Priority.** P1: a Done-when item, a risk of losing or duplicating data, a security property, or a core state machine. P2: another explicit spec rule, or a second partition of a P1 behavior. P3: a boundary that is costly to set up, or a detail close to cosmetic.
- **What runs.** Playwright on WebKit drives the built UI against the real Rust core through the web bridge, and the core talks to the in-process fake Gmail. The native shell is not exercised: a recording platform stands in for notifications, the Dock badge, opened URLs and files, and stored tokens.
- **Harness.** Seed scenarios are in `e2e/seeds.ts`, the page object and app time in `e2e/app.ts`, and the fake's control API (faults, holds, the request log) in `e2e/control.ts`. `e2e/harness.spec.ts` shows each piece in use.
- **The check.** `e2e/plan.spec.ts` checks that every case line parses, ids are unique and sit in their file's group, every § names a SPEC heading, every case has a test in its file, every test id is a case, and the lists below name only real cases and files.

## Cases

### accounts.spec.ts (ACC)

- ACC-01 · P1 · First run adds the first account, and its token goes to a file only the user can read · §Accounts, §Security, §Done when
- ACC-02 · P1 · Add a second account from the palette, then switch with `Ctrl+1` and `Ctrl+2` · §Accounts, §Switching, §Look, §Done when
- ACC-03 · P1 · Switching closes overlays and returns each account to its own place · §Switching
- ACC-04 · P1 · Switching with an open composer works like a second `Esc` · §Switching, §Keyboard
- ACC-05 · P2 · Avatar unread counts · §Switching
- ACC-06 · P1 · Accounts off screen keep syncing, notifying, and sending · §Switching, §Send and undo
- ACC-07 · P1 · Remove an account · §Settings, §Accounts
- ACC-08 · P1 · `invalid_grant` signs the account out; the queue and outbox wait, then recover · §Accounts, §Gmail errors
- ACC-09 · P2 · A failed sign-in adds nothing · §Accounts
- ACC-10 · P2 · Accounts with the same Gmail thread id stay separate · §Data
- ACC-11 · P1 · A task left from before Sign in again does not sign the account out again · §Accounts, §Gmail errors
- ACC-12 · P2 · Re-adding an account while its removal is still revoking keeps the new sign-in · §Accounts, §Settings
- ACC-13 · P1 · Adding an account that is already signed in replays its queue once · §Accounts, §Offline
- ACC-14 · P1 · An account with no stored token at launch shows Sign in again · §Accounts
- ACC-15 · P2 · Clicking the account on screen opens Settings; another account switches · §Switching
- ACC-16 · P1 · A removal waiting on its revoke holds up no other account · §Accounts
- ACC-17 · P1 · A first sign-in shows the whole Inbox before reading the account's drafts · §Sync
- ACC-18 · P1 · An account removed while its Sign in again waits in the browser comes back and syncs · §Accounts
- ACC-19 · P1 · A reply open when its account signs out stays on screen until it is closed · §Accounts, §Keyboard
- ACC-20 · P1 · Removing the account on screen while its reply is open leaves no composer behind · §Accounts, §Switching
- ACC-21 · P2 · Removing the account on screen shows the next account on the rail, or the one before the last · §Accounts, §Settings
- ACC-22 · P1 · A sign-in left waiting while the user moves on finishes in the background, with what changed in Gmail meanwhile · §Accounts, §Switching
- ACC-23 · P2 · Starting another sign-in cancels the waiting one, which shows no error · §Accounts
- ACC-24 · P2 · A sign-in cut off by a lost connection or a quit adds nothing, and the next one works · §Accounts

### inbox-navigation.spec.ts (NAV)

- NAV-01 · P1 · Inbox rows show the sender, subject — snippet, time, and an unread mark · §Window, §Views, §Layout
- NAV-02 · P1 · `j` and `k` move the highlight across split boundaries · §Window, §Keyboard, §Done when
- NAV-03 · P1 · `Enter` opens a conversation, `Esc` returns with the highlight; a click opens too · §Window, §Done when
- NAV-04 · P1 · `j` and `k` inside a conversation open the next and previous conversation · §Window, §Reading
- NAV-05 · P2 · `g` jumps and the rail · §Views, §Window, §Labels
- NAV-06 · P2 · Empty states · §Window
- NAV-07 · P2 · The highlight moves when a poll removes the highlighted row · §Keyboard
- NAV-08 · P2 · `j` keeps the prefetched neighbors it still needs · §Performance, §Subscriptions
- NAV-10 · P1 · The highlight follows the mouse, so `e` acts on the row under it · §Layout, §Keyboard
- NAV-11 · P1 · A poll that empties the list clears the highlight, so `#` acts on the mail that arrives next · §Keyboard
- NAV-12 · P1 · A row removed while another account is on screen moves that account's highlight · §Switching, §Keyboard
- NAV-13 · P1 · `g #`, `g !`, and a held `g` jump as typed on a keyboard, Shift and repeats included, and touch no mail · §Views, §Keyboard
- NAV-14 · P2 · A list scrolled away from the highlight stays put while mail changes; moving the highlight, even with repeated `j` past the bottom edge, keeps it in view · §Layout, §Keyboard

### views.spec.ts (VIEW)

- VIEW-01 · P2 · A thread's latest message sets its row · §Views
- VIEW-02 · P1 · View membership comes from message labels · §Views
- VIEW-03 · P1 · Done lists every thread outside Inbox, from Gmail too · §Views
- VIEW-04 · P1 · Drafts lists every draft, and mail actions do nothing there · §Views, §Subscriptions
- VIEW-05 · P2 · Label, Trash, and Spam views also list from Gmail · §Views
- VIEW-06 · P1 · All Mail lists every thread outside Trash and Spam, from Gmail too · §Views, §Keyboard

### conversation.spec.ts (CONV)

- CONV-01 · P1 · A long thread opens with read messages collapsed and folded, scrolled to the first unread · §Reading, §Done when
- CONV-02 · P2 · Fold boundary: four collapsed messages show, five fold · §Reading
- CONV-03 · P2 · Expansion is fixed when the conversation opens · §Subscriptions
- CONV-04 · P2 · Expanding, collapsing, and selecting with the mouse · §Reading
- CONV-05 · P2 · `n`, `p`, and `o` select and expand messages · §Reading
- CONV-06 · P2 · `Shift+O` expands everything; pressing it again restores the default · §Reading
- CONV-07 · P2 · Quoted history sits behind `•••` · §Reading
- CONV-08 · P1 · Opening marks the thread read locally and on Gmail · §Reading
- CONV-09 · P2 · A message that arrives while the conversation is open · §Reading, §Subscriptions
- CONV-10 · P2 · If the selected message leaves, the latest is selected · §Reading
- CONV-11 · P2 · Messages in trash or spam are hidden outside Trash and Spam · §Reading
- CONV-12 · P2 · `Tab` cycles links, `Enter` opens one outside the app, `Esc` clears it · §Reading, §Keyboard
- CONV-13 · P2 · Shortcuts still work with focus inside a message body · §Reading
- CONV-14 · P3 · Message iframes size to their content · §Reading
- CONV-15 · P2 · A conversation left before it loaded is not marked read · §Reading
- CONV-16 · P1 · The conversation that opens after an action is marked read · §Reading, §Keyboard
- CONV-17 · P2 · Message headers name people as Superhuman does, and the selected message is a raised card · §Reading, §Layout
- CONV-18 · P2 · The subject row's Done and Snooze buttons act like `e` and `h` · §Layout, §Keyboard
- CONV-19 · P1 · A message's dark-mode styles stay off when macOS is in dark mode · §Reading, §Look
- CONV-20 · P2 · A conversation opened, closed, and opened again in one batch still reads its body · §Sync, §Actions
- CONV-21 · P1 · A message still downloading shows its snippet and a loading bar, then its body · §Reading, §Subscriptions
- CONV-22 · P1 · A message labelled with a legacy charset but served as UTF-8 reads correctly · §Reading
- CONV-23 · P2 · An address added to the account in Gmail is Me in a conversation already open · §Reading, §Sync
- CONV-24 · P2 · `r`, `Enter`, and `f` do nothing in a conversation that is gone · §Reading, §Reply composer
- CONV-25 · P1 · A reply open when its conversation turns gone closes as a second `Esc` would, keeping its text · §Reply composer, §Keyboard

### triage-actions.spec.ts (TRI)

- TRI-01 · P1 · `e` on the list removes the row before Gmail answers, moves the highlight, and archives every message of the thread · §Archive, trash, star, read, §Keyboard, §Done when
- TRI-02 · P1 · `e` in an open conversation opens the next one, or the previous one, or returns to the empty list · §Keyboard
- TRI-03 · P1 · `#` moves to Trash and never touches draft messages · §Archive, trash, star, read, §Done when
- TRI-04 · P2 · `!` moves to Spam · §Archive, trash, star, read
- TRI-05 · P1 · Star and read toggles · §Archive, trash, star, read, §Done when
- TRI-06 · P1 · `x` and `Shift+J` / `Shift+K` select; actions apply to the whole selection as one undo entry · §Keyboard, §Undo
- TRI-07 · P2 · `s` and `u` on a mixed selection follow the `l` rule · §Keyboard
- TRI-08 · P1 · `Shift+E` returns to Inbox from any view, and the row leaves at once · §Archive, trash, star, read, §Snooze
- TRI-09 · P2 · `Esc` closes the open overlay before it clears a selection · §Keyboard
- TRI-10 · P2 · Each action removes its row from the right lists · §Actions
- TRI-11 · P1 · An action changes only the messages it recorded; a reply arriving meanwhile stays in Inbox · §Archive, trash, star, read
- TRI-12 · P1 · `e` marks the conversation read, and `z` makes the same messages unread again · §Archive, trash, star, read
- TRI-13 · P1 · `Cmd+A` selects from the highlight to the end of its split; `Cmd+Shift+A` the whole split · §Keyboard
- TRI-14 · P1 · A selected row that a poll removes is not acted on · §Keyboard
- TRI-15 · P1 · `s`, `u`, and `l` on Select all count every selected conversation, shown or not · §Keyboard, §Actions
- TRI-16 · P1 · `Shift+E` on mail in Trash or Spam reached by search takes it out of Trash or Spam · §Archive, trash, star, read, §Snooze
- TRI-17 · P1 · `j` on the last conversation and `k` on the first keep the selection · §Keyboard

### undo.spec.ts (UND)

- UND-01 · P1 · `z` walks back through the stack · §Undo, §Actions
- UND-02 · P1 · Undo reverts only the labels the action changed · §Undo
- UND-03 · P1 · Actions that make no entry are skipped by `z` · §Undo, §Settings
- UND-04 · P1 · Each account has its own undo history · §Undo
- UND-05 · P2 · A quick `e` followed by `z` `z` undoes the right entries · §Actions
- UND-06 · P2 · `z` still works after a restart · §Undo
- UND-07 · P3 · Only the last 50 entries are kept · §Undo
- UND-08 · P2 · Undoing a trash does not bring back a reply whose send was undone · §Undo

### labels.spec.ts (LAB)

- LAB-01 · P1 · `l` adds and removes a user label · §Labels, §Done when
- LAB-02 · P2 · `l` on a selection: remove only if every thread has the label · §Labels
- LAB-03 · P2 · Create a label from the picker and apply it in one action · §Labels, §Actions
- LAB-04 · P1 · A label created offline is mapped to its Gmail id when the queue replays · §Offline
- LAB-05 · P2 · If `labels.create` says the name exists, the existing label is used · §Labels, §Gmail errors
- LAB-06 · P2 · `y` removes the current view's label, where it can · §Labels
- LAB-07 · P2 · A label first seen in history is picked up · §Sync
- LAB-08 · P2 · A label list read before a create landed does not delete the new label · §Sync, §Labels

### search.spec.ts (SRC)

- SRC-01 · P1 · `/` shows local results at once; focus moves to results; `Esc` restores the previous view · §Search, §Done when
- SRC-02 · P2 · Local query language, and what is sent to Gmail · §Search
- SRC-03 · P1 · Gmail hits are merged in as their metadata arrives · §Search
- SRC-04 · P2 · The Gmail query waits for a 300 ms pause · §Search
- SRC-05 · P1 · Offline search shows cached mail and says the results are from this Mac · §Search, §Offline, §Done when
- SRC-06 · P2 · Actions on search results follow the removal table · §Actions
- SRC-07 · P3 · Gmail results come 15 at a time, and each page is asked for once · §Search, §Views
- SRC-08 · P2 · A sent reply is found by local search · §Search, §Send and undo
- SRC-09 · P2 · Search takes the header as in Superhuman, marks matches, and lists tips · §Search
- SRC-10 · P2 · A Gmail search stops searching when its account signs in again under it · §Search, §Accounts
- SRC-11 · P1 · Local search applies Gmail's operators before Gmail answers · §Search
- SRC-12 · P2 · A query local search can't evaluate waits for Gmail; offline it says the search needs Gmail · §Search, §Offline
- SRC-13 · P2 · The next Gmail page loads near the oldest Gmail hit, not the list's end · §Search, §Views, §Subscriptions
- SRC-14 · P1 · A search or Gmail-listed view opened offline lists from Gmail when the network returns · §Search, §Views, §Offline
- SRC-15 · P1 · An open search runs once Gmail can be reached again, though nothing else calls Gmail · §Search, §Offline
- SRC-16 · P1 · A conversation opened from a search in Trash or Spam lists their messages, by `Enter` and `j` · §Search, §Reading

### splits.spec.ts (SPL)

- SPL-01 · P1 · Default routing to Important and Other, with counts in the header · §Split Inbox, §Done when
- SPL-02 · P1 · `Tab` and `Shift+Tab` move between visible splits · §Split Inbox, §Done when
- SPL-03 · P1 · Pin precedence: sender over domain over label · §Where a conversation lands
- SPL-04 · P1 · Pinning from the palette replaces an older pin; `z` restores it; a sender pin holds across syncs · §Split Inbox, §Undo, §Done when
- SPL-05 · P2 · Your own reply does not move a thread · §Where a conversation lands
- SPL-06 · P1 · Custom splits: add, order, rename, turn off, delete · §Split Inbox, §Custom rules
- SPL-07 · P2 · Starter splits, and a flat inbox · §Custom rules
- SPL-08 · P1 · Empty splits hide themselves, except after `g` `i` / `g` `o` · §Split Inbox, §Views, §Done when
- SPL-09 · P3 · Counts cap at `999+` · §Split Inbox
- SPL-10 · P2 · Clicking a split name scrolls to its section · §Window
- SPL-11 · P1 · A rule the core refuses keeps the sheet open with every edit · §Custom rules

### snooze.spec.ts (SNZ)

- SNZ-01 · P1 · The snooze picker at a normal hour · §Snooze, §Done when
- SNZ-02 · P3 · "Later today" disappears at 18:00 · §Snooze
- SNZ-03 · P1 · Snoozing takes the thread out of Inbox, here and in Gmail · §Snooze, §Done when
- SNZ-04 · P1 · Snoozed threads wake on time, on launch, and after sleep, even offline · §Snooze
- SNZ-05 · P1 · `INBOX` added from outside clears a snooze; your own reply does not · §Snooze
- SNZ-06 · P1 · The app's own label changes, echoed by history, do not clear a newer snooze · §Snooze
- SNZ-07 · P1 · Undo, `Shift+E`, `e`, `#`, and `!` on snoozed threads · §Snooze, §Undo
- SNZ-08 · P2 · `Enter` before the typed time's options arrive snoozes until that time · §Snooze

### compose-reply.spec.ts (REP)

- REP-01 · P1 · `r` on the list opens the reply composer under the thread, in view below a long message, ready to type · §Reply composer, §Done when
- REP-02 · P1 · What `r`, `Enter`, and `f` do on the list and in a conversation · §Reply composer, §Keyboard
- REP-03 · P1 · Reply and reply-all addressing · §Addressing
- REP-04 · P1 · A sent reply stays on the thread and carries Gmail's quote · §Addressing, §Send and undo
- REP-05 · P1 · The reply follows the selected message · §Addressing
- REP-06 · P1 · Forward quotes the target, carries its attachments, and stays on the thread · §Addressing
- REP-07 · P1 · Which draft `r`, `Enter`, and `f` resume · §Reply composer
- REP-08 · P1 · A half-written reply survives new replies · §Reply composer
- REP-09 · P2 · Drafts from Gmail get a kind and a target · §Reply composer
- REP-10 · P1 · Closing and sending the reply composer keep the conversation open · §Reply composer
- REP-11 · P2 · A draft read from Gmail is split at its `gmail_quote` · §Addressing
- REP-12 · P2 · A Gmail draft's attachments are downloaded before its first upsert · §Addressing
- REP-13 · P1 · Reply keys pressed before the conversation loads open one composer · §Keyboard
- REP-14 · P2 · Compose keys work while focus is inside a message · §Keyboard
- REP-15 · P2 · Text typed before the reply opens goes into the reply · §Keyboard, §Reply composer
- REP-16 · P1 · A reply started while the message's body downloads quotes all of it · §Addressing
- REP-17 · P1 · A forward started while the message's body downloads carries all of it and its attachments · §Addressing
- REP-18 · P2 · A forwarded attachment that arrives with the body can be removed, and an untouched forward is still discarded · §Addressing, §Actions
- REP-19 · P1 · A reply discarded before its target's body or its own first load arrives leaves nothing on Gmail · §Drafts, §Addressing

### compose-new.spec.ts (NEW)

- NEW-01 · P1 · `c` replaces the screen, and closing it returns to where it came from · §Reply composer, §Keyboard, §Done when
- NEW-02 · P1 · Compose and send a formatted message, including the last keystrokes · §Compose, §Send and undo, §UI state
- NEW-03 · P2 · TipTap shortcuts that clash with the compose table are replaced · §Compose
- NEW-04 · P2 · Compose field shortcuts work whether the editor is focused or blurred · §Compose
- NEW-05 · P1 · `Esc` in compose: the first blurs, the second closes; while blurred, only a few keys work · §Keyboard
- NEW-06 · P1 · Shortcuts never fire while typing in compose · §Keyboard
- NEW-07 · P1 · A fresh composer closed untouched leaves nothing behind · §Keyboard
- NEW-08 · P2 · `Cmd+K` in compose · §Keyboard, §Compose
- NEW-09 · P1 · Sending with no recipients never loses the draft; text still in the To field is sent, even with the suggestion list open · §Recipients and autocomplete
- NEW-10 · P2 · From picker and the sending account · §Addressing, §Switching
- NEW-11 · P1 · `Cmd+Shift+,` discards locally and in Gmail · §Compose, §Drafts
- NEW-12 · P2 · Opening another composer, closing search, a new query, or another account's Split Inbox settings first closes the composer, as a second `Esc` would · §Actions, §Search, §Settings
- NEW-14 · P1 · An `Esc` pressed just after a composer opens, or after a key that moves focus in compose, counts, however late the focus would come · §Keyboard

### recipients.spec.ts (RCP)

- RCP-01 · P1 · Autocomplete: at most six suggestions, keyboard choice, `Esc` stays in the field · §Recipients and autocomplete, §Done when
- RCP-02 · P1 · Ranking and exclusions come from synced mail · §Recipients and autocomplete
- RCP-03 · P2 · Ways to turn typed text into a chip · §Recipients and autocomplete
- RCP-04 · P2 · `Backspace` and paste · §Recipients and autocomplete
- RCP-06 · P2 · `Tab` moves between fields unless the suggestion list is open · §Keyboard
- RCP-07 · P2 · Cc and Bcc stay hidden until used · §Recipients and autocomplete
- RCP-08 · P2 · New mail updates the suggestions · §Recipients and autocomplete
- RCP-09 · P2 · Suggestions that answer after a chip is made stay closed · §Recipients and autocomplete
- RCP-10 · P2 · The autosave leaves an address being typed alone · §Recipients and autocomplete, §Drafts
- RCP-11 · P1 · Every address the core would refuse shows in `danger` and blocks Send until it is fixed · §Recipients and autocomplete
- RCP-12 · P2 · A chip removed while an address is being typed stays removed · §Recipients and autocomplete

### attachments.spec.ts (ATT)

- ATT-01 · P1 · Attach files and send them · §Drafts, §Compose
- ATT-02 · P1 · Received attachments download only when opened · §Reading
- ATT-03 · P1 · A forward made offline still sends its attachments · §Addressing, §Send and undo
- ATT-04 · P2 · MIME tree shape and encoded headers · §Drafts
- ATT-05 · P1 · A send while a file is still attaching carries the file · §Compose, §Drafts
- ATT-06 · P1 · Closing a fresh composer while a file is attaching keeps the draft · §Keyboard, §Drafts
- ATT-07 · P1 · A downloaded attachment is quarantined before it opens · §Reading
- ATT-08 · P2 · A file over 25 MB is refused before it is read; one of 25 MB is taken · §Drafts
- ATT-09 · P1 · `Cmd+Q` while a file is attaching quits once the composer's close is sent · §Actions, §Drafts
- ATT-10 · P1 · A send waiting for a file still goes when the composer is closed meanwhile; a file that fails keeps the draft · §Drafts
- ATT-11 · P1 · A draft reopened while its close waits for a file keeps the new text and the file · §Drafts, §Actions
- ATT-13 · P1 · A file Gmail serves inside the message opens and forwards · §Reading, §Addressing
- ATT-14 · P1 · A file attached while a send waits for another goes with the send · §Drafts
- ATT-15 · P1 · Discarding while a send waits for a file cancels the send · §Drafts
- ATT-16 · P2 · The same file can be attached twice · §Drafts
- ATT-17 · P2 · An attachment that can't be opened says why · §Reading, §Offline
- ATT-18 · P1 · A file sent inline, as Apple Mail sends it, or an image the body doesn't show is an attachment, and a forward carries it · §Reading, §Addressing
- ATT-19 · P1 · An address typed but not yet a chip goes with a close or send that waits for a file · §Drafts, §Recipients and autocomplete
- ATT-20 · P1 · Removing the account while its composer's send or close waits for a file leaves nothing behind · §Accounts, §Drafts

### drafts.spec.ts (DRF)

- DRF-01 · P1 · Drafts autosave and update a single Gmail draft · §Drafts
- DRF-02 · P1 · `Enter` in Drafts opens a draft according to its kind · §Reading
- DRF-03 · P1 · Edits made to a draft in Gmail, with the draft closed or open · §Drafts
- DRF-04 · P1 · A draft deleted or sent elsewhere · §Drafts
- DRF-05 · P1 · Discarding while offline is queued and replayed in order · §Drafts, §Offline
- DRF-06 · P1 · A draft written offline reaches Gmail exactly once · §Drafts, §Offline
- DRF-07 · P2 · Sending cancels a queued upsert · §Drafts
- DRF-08 · P2 · A webview reload or a crash with a composer open · §Subscriptions
- DRF-09 · P3 · The app's own drafts are read back after the account is re-added · §Addressing, §Gmail errors
- DRF-10 · P1 · Text deleted while its save is on the way stays deleted · §Actions
- DRF-11 · P1 · Typing before a reply's first load keeps its filled fields · §Actions
- DRF-12 · P1 · An edit saved while the draft's upsert is in flight still reaches Gmail · §Drafts
- DRF-13 · P1 · A draft discarded while its first upsert is in flight stays gone · §Drafts, §Gmail errors
- DRF-14 · P1 · A second Gmail draft carrying a local draft's id is deleted as an extra copy · §Gmail errors
- DRF-15 · P1 · A draft reconcile on a stale list keeps a draft written meanwhile · §Drafts, §Sync
- DRF-16 · P1 · A Gmail edit read back after the draft was reopened and edited does not replace the edit · §Drafts
- DRF-18 · P1 · A crash after `drafts.create` reached Gmail, before its ids were stored, leaves one draft · §Gmail errors, §Drafts
- DRF-19 · P1 · An edit still waiting to reach Gmail is not replaced by an older Gmail version · §Drafts
- DRF-20 · P1 · `j` and `k` in a conversation opened from Drafts open the other drafts' threads and keep the Drafts highlight · §Reading, §Keyboard
- DRF-21 · P1 · A draft deleted on Gmail while an edit waits to reach Gmail is written again with the edit · §Drafts
- DRF-22 · P2 · A draft deleted elsewhere just before its composer loads leaves the screen and says so · §Drafts

### undo-send.spec.ts (SND)

- SND-01 · P1 · The send waits ten seconds, then goes out once · §Send and undo, §Done when
- SND-02 · P1 · Undo while waiting cancels the send and reopens the composer · §Send and undo, §Done when
- SND-03 · P1 · `z` reaches a send only while it is the newest entry; the toast's Undo always does · §Undo, §Send and undo
- SND-04 · P1 · Undo after the send was accepted trashes only that message · §Send and undo
- SND-05 · P1 · Undo while sending is held until the send resolves · §Send and undo
- SND-06 · P1 · A rejected send brings the draft back · §Send and undo, §Gmail errors
- SND-07 · P1 · Offline at send time: "Email delayed", then sent once online · §Send and undo, §Offline, §Done when
- SND-08 · P1 · `drafts.send` with no clear answer never sends twice · §Send and undo, §Gmail errors
- SND-09 · P1 · Outbox rows across a quit and relaunch · §Send and undo, §Undo
- SND-10 · P2 · Send toasts stay across accounts · §Switching
- SND-11 · P2 · Undoing a waiting send while another composer is open · §Send and undo
- SND-13 · P1 · A draft reconcile in the undo window keeps the version that was sent · §Drafts, §Send and undo
- SND-14 · P1 · A crash while the send writes the draft still sends the final version · §Send and undo
- SND-15 · P1 · A held undo resolves at once when the send could not reach Gmail · §Send and undo
- SND-16 · P1 · A send toast closed before the send went out takes its undo entry with it once sent · §Undo
- SND-17 · P1 · An Undo that reaches the core only after the send went out opens no composer · §Send and undo
- SND-18 · P1 · A send while the draft's first upsert is in flight goes out once and leaves no draft · §Drafts, §Gmail errors
- SND-19 · P1 · A send the core refuses says it was not sent and brings the draft back · §Send and undo, §Actions
- SND-20 · P2 · An Undo the core refuses says why and opens no composer · §Send and undo, §Actions
- SND-21 · P1 · A held undo resolves at once when the send's access token could not be refreshed · §Send and undo, §Gmail errors
- SND-22 · P1 · A sent reply stays on the thread, marked Sent, until its message lands without waiting for a poll · §Reply composer, §Send and undo
- SND-23 · P1 · A send settled as sent with no message to show leaves the thread once its toast closes · §Reply composer, §Send and undo
- SND-24 · P1 · A send whose account signs out or is removed while `drafts.send` is on its way · §Send and undo, §Accounts

### offline.spec.ts (OFF)

- OFF-01 · P1 · Airplane mode: read, search, autocomplete, and act; it all lands when the network returns · §Offline, §Done when
- OFF-02 · P1 · Every kind of queued action replays in order · §Offline
- OFF-03 · P1 · The queue survives a restart · §Offline
- OFF-04 · P1 · A queued action wins on the messages it recorded, without hiding newer mail · §Offline, §Archive, trash, star, read
- OFF-07 · P1 · A Gmail answer read before an action landed does not undo it · §Offline, §Sync
- OFF-05 · P1 · A rejection during replay rolls back that one action, and replay continues · §Offline, §Gmail errors
- OFF-06 · P2 · Losing the network during replay leaves the rest queued · §Offline
- OFF-08 · P2 · Conversations opened offline read only the bodies still on screen when the network returns · §Sync, §Offline
- OFF-09 · P1 · A poll during the replay of a queued action keeps the action applied · §Offline, §Archive, trash, star, read

### gmail-errors.spec.ts (ERR)

- ERR-01 · P1 · Transient failures retry silently and keep the row as the user left it · §Gmail errors
- ERR-02 · P2 · Back-off timing and `Retry-After` · §Gmail errors
- ERR-03 · P2 · A quiet sync status after 30 s of retrying · §Gmail errors
- ERR-04 · P1 · A rejection rolls the action back and shows Gmail's error · §Gmail errors, §Archive, trash, star, read
- ERR-05 · P1 · An expired token is refreshed once, and the request is retried · §Gmail errors
- ERR-06 · P1 · A 404 on part of an action drops only the missing messages · §Gmail errors
- ERR-07 · P1 · `drafts.create` with no clear answer never makes a duplicate draft · §Gmail errors
- ERR-08 · P2 · A 404 while fetching a body is dropped silently · §Gmail errors
- ERR-09 · P3 · A rejected undo or label create · §Gmail errors
- ERR-10 · P2 · The quiet sync status clears once Gmail answers after a relaunch · §Gmail errors

### sync.spec.ts (SYN)

- SYN-01 · P1 · The connect sync matches Gmail · §Sync, §Done when
- SYN-02 · P1 · Mail that changed while the app was closed shows after relaunch · §Sync, §Done when
- SYN-03 · P1 · Polls bring in changes made elsewhere · §Sync
- SYN-04 · P1 · History too old: a full resync that sends no notifications · §Sync
- SYN-05 · P3 · The full resync keeps a thread that moved during the listing · §Sync
- SYN-06 · P2 · `sendAs` and labels refresh on focus · §Sync
- SYN-07 · P3 · An opened thread fetches its bodies right away · §Sync
- SYN-08 · P1 · A slower thread fetch never undoes a newer one · §Sync, §Data
- SYN-09 · P2 · A connect sync retried after a failure misses no change made in between · §Sync
- SYN-11 · P2 · A slower label or `sendAs` read never undoes a newer one · §Sync

### notifications.spec.ts (NOT)

- NOT-01 · P1 · New mail in either account raises a notification and adds to the Dock badge · §Notifications, §Done when
- NOT-02 · P1 · When a notification is not raised · §Notifications
- NOT-03 · P1 · More than three new messages in one poll become one summary · §Notifications
- NOT-04 · P1 · A muted account does not notify and is left out of the badge · §Notifications, §Settings
- NOT-05 · P1 · The Dock badge counts unread Inbox threads · §Notifications
- NOT-06 · P2 · Clicking a notification opens that account's Inbox on the thread · §Notifications

### settings.spec.ts (SET)

- SET-01 · P2 · What the Settings sheet holds, and how it opens · §Settings, §Keyboard
- SET-02 · P1 · Remote images · §Reading, §Settings
- SET-03 · P2 · Hint text is 11px, the shortcut chips are readable, and the shortcut sheet's rows never overlap · §Look
- SET-05 · P1 · With remote images off, nothing remote loads in a message or a reply's quote · §Reading, §Settings
- SET-06 · P2 · `Esc` closes a sheet from one of its fields, and a sheet open over a composer · §Keyboard

### palette.spec.ts (PAL)

- PAL-01 · P1 · Open, filter, run, and close the palette · §Keyboard, §Done when
- PAL-02 · P2 · Palette commands do what their keys do · §Keyboard
- PAL-03 · P1 · The palette lists only the commands that work where it opened; the shortcut sheet lists every shortcut · §Keyboard, §Accounts, §Layout
- PAL-04 · P2 · An empty Inbox offers no mail actions and hints only the palette · §Keyboard, §Layout

### security.spec.ts (SEC)

- SEC-01 · P1 · Hostile HTML cannot run script or navigate · §Reading, §Security, §Done when
- SEC-02 · P1 · Mail text never becomes markup outside the frame · §Security
- SEC-03 · P1 · Links never navigate the app window · §Reading
- SEC-04 · P2 · The webview holds no Gmail credentials · §Security
- SEC-06 · P1 · The UI runs under the release app's content security policy · §Security, §Testing
- SEC-07 · P1 · Links in a reply's quoted text open in the browser, never in the composer · §Reading, §Addressing

### ui-consistency.spec.ts (UIC)

- UIC-01 · P1 · Rows you just acted on never flash back during fast triage or a poll · §Actions
- UIC-02 · P2 · A reload keeps queued actions and keeps action numbers going · §Actions
- UIC-03 · P2 · An action whose submit fails is sent again · §Actions
- UIC-04 · P2 · A row that changed while no list held it is not shown from an old copy · §Subscriptions
- UIC-05 · P1 · `Cmd+Q` quits only once the composer's close has committed · §Actions
- UIC-06 · P1 · An action the core rejects tells the user why · §Actions
- UIC-07 · P1 · An action the core can't read is refused, and the actions after it still apply · §Actions
- UIC-08 · P1 · An action stays applied when the one before it is acked · §Actions

### unwanted.spec.ts (UNW)

- UNW-01 · P1 · `Cmd+U` unsubscribes with one click when the list offers it · §Unsubscribe and block
- UNW-02 · P1 · An email unsubscribes, and "mark done all" archives the sender's Inbox mail · §Unsubscribe and block
- UNW-03 · P1 · A list with only a web page opens it, and "trash all" trashes the sender's mail · §Unsubscribe and block
- UNW-04 · P2 · No unsubscribe link, or no network, says so; the archive still happens · §Unsubscribe and block
- UNW-05 · P1 · Block makes a Gmail filter to Trash, trashes the conversation, and keeps new mail out · §Unsubscribe and block
- UNW-06 · P2 · Blocked senders lists the blocks, and Unblock deletes the filter · §Unsubscribe and block
- UNW-07 · P1 · An account signed in before filters were asked for is offered Sign in again · §Unsubscribe and block, §Accounts
- UNW-08 · P1 · Mark spam, and block the domain · §Unsubscribe and block
- UNW-09 · P1 · Unsubscribe, block, and unblock act only on the account's own thread and block filter · §Unsubscribe and block
- UNW-10 · P1 · Block and unsubscribe act on their own account when another one is on screen by then · §Unsubscribe and block, §Switching

## Not covered end-to-end, and why

- Window behavior (a second launch focuses the window, closing hides it, a Dock click shows it, size and position restored but not visibility, the window shown only once the page connects or after 5 s, the overlay title bar) and quitting from the menu bar or macOS: the native shell is not exercised. The suite's quit is the E2E server asking the page itself (UIC-05, ATT-09); `tests/menu.rs` checks that the menu bar's Quit is the app's own. Checked by hand on a real Mac with the release build, on a test account:
  - type in a composer and `Cmd+Q` within 0.5 s: the text is there on relaunch, and in the Gmail draft;
  - the same with the window hidden (`Cmd+W`) and with it minimized;
  - archive, `Cmd+Q` at once: archived on relaunch, and in Gmail once online;
  - archive, then Quit in the Dock at once: archived on relaunch;
  - in a debug build, with the page paused at a breakpoint in the Web Inspector: the app quits 10 s after `Cmd+Q`.
- Real Google OAuth (the system browser, the unverified-app warning, a production consent screen): the fake OAuth stands in, and ACC-01 checks the token file and the auth URL's scope, PKCE, and loopback. Checked by hand against two real accounts (§Testing).
- Performance budgets, the keydown-to-paint meter, 60 fps scrolling, cold launch: CI WebKit is not the M1 target. Checked on the perf fixture (§Performance, `crates/ub-core/src/tests/perf.rs`).
- Look, Motion, and Reduce Motion: visual, not behavior; SET-03 checks the hint size and the shortcut sheet. Design review.
- The quota bucket, the six priorities, and at most 8 requests per account: internal scheduling, not stable through the UI; SYN-07 is a smoke check. `crates/ub-gmail/tests/gmail/scheduler.rs`.
- The back-off jitter: random; ERR-02 checks `Retry-After` and that gaps grow. `crates/ub-gmail/tests/gmail/scheduler.rs`.
- OAuth protocol edges (a wrong `state`, concurrent 401s sharing one refresh, refresh before expiry, secrets out of debug output): below the UI. `crates/ub-gmail/tests/gmail/oauth.rs`.
- How each HTTP response maps to an outcome: the ERR cases and SND-08 cover each outcome's behavior; the mapping is implied.
- The sanitizer policy, element by element: SEC-01 checks a hostile sample end to end. `crates/ub-mime/tests/sanitize.rs`.
- MIME parsing robustness (charsets, malformed headers, random input): fuzzing, not E2E; ATT-04 has one encoded-subject round trip. `crates/ub-mime/tests/no_panic.rs`.
- The query language's grammar, errors, FTS translation, and Unicode matching: pure parsing; SRC-02, SRC-11, SRC-12, and SPL-06 check one query per family and the wiring. `crates/ub-domain/src/query/tests.rs`.
- Snooze times across weekdays and DST, and the phrase grammar: pure; SNZ-01 checks a normal hour and SNZ-02 the 18:00 boundary. `crates/ub-domain/src/snooze/options.rs`.
- Contact scores (recency halving every 90 days, word-start matching): pure; RCP-02 checks tier order and exclusions from real mail. `crates/ub-domain/src/contacts.rs`.
- Person-to-person routing over every label combination: pure; SPL-01 checks the list flag and four categories. `crates/ub-domain/src/tests.rs`.
- Writer and channel ordering (`rev`, `ack`, patches against first loads): races E2E can't force; UIC-01 checks the visible no-flash rule. `crates/ub-core/src/tests/writer_channel.rs`.
- The mirror's re-application of pending actions in detail: UND-05, UIC-01, TRI-01, and TRI-10 cover the behavior; exact `ack` interleavings can't be forced E2E.
- Table schemas and `rev` and action-number values: internal.
- A due snooze not yet woken staying in Snoozed: true only while the app is not running; SNZ-04 checks that Gmail keeps it out of Inbox meanwhile and that it wakes before the first paint.
- The full resync's step-3 race at scale, billing and daily units: cost and scale facts.
- Real sleep and wake: simulated by `app.sleep` and `app.sleepAndWake`.
- Label colors, nested-label editing, permanent delete: out of scope (§Non-goals, §Labels).

## Kept unit and property tests

Unit tests only for complex algorithms, security properties, and races the suite can't reach (AGENTS.md); the suite doesn't repeat their partitions. Paths are under `src-tauri/`.

- `crates/ub-domain/src/query/tests.rs`: lenient and strict parsing and its errors, operator case and precedence, search operators and scope, split rules refusing search terms, FTS5 translation and escaping, `is:list` stripped for Gmail (E2E: SRC-02, SRC-11, SRC-12, SPL-06).
- `crates/ub-domain/src/tests.rs`: person-to-person over categories and list mail, the starters, pin precedence, the latest message from someone else (SPL-01, SPL-03, SPL-07).
- `crates/ub-domain/src/snooze/options.rs` and `crates/ub-domain/src/snooze/text.rs`: options before and after 18:00 and on every weekday, DST gaps and folds, the phrase grammar (SNZ-01, SNZ-02).
- `crates/ub-domain/src/contacts.rs`: autocomplete matching, ranking, and no-reply addresses.
- `crates/ub-mime/tests/sanitize.rs`: the hostile fixture, what is kept and dropped, `rel=noopener`, `cid:` to `data:`, text extraction (SEC-01).
- `crates/ub-mime/tests/payload.rs`: a charset with no decoder read as undeclared text; the attachments inside an attached message, and not its body or inline images; an image is inline only when the HTML body shows it, by a plain or percent-encoded `cid:` reference, here and inside an attached message, and only those are fetched for the body (ATT-18).
- `crates/ub-mime/tests/no_panic.rs`: 3,000 seeded random inputs through every parser and the sanitizer.
- `crates/ub-mime/tests/addresses.rs`: `is_valid_email` against the address table the composer's copy of the rule is held to (RCP-11).
- `crates/ub-mime/src/unsubscribe.rs`: the `List-Unsubscribe` parser and when one-click applies.
- `crates/ub-mime/src/outgoing.rs`: user text can't add headers to an outgoing message.
- `crates/ub-gmail/tests/gmail/scheduler.rs`: priorities, concurrency, quota, back-off, and a removed account starting afresh.
- `crates/ub-gmail/tests/gmail/oauth.rs`: the `state` check, one refresh shared by concurrent 401s, secrets out of debug output.
- `crates/ub-gmail/tests/gmail/client.rs`: one-click unsubscribe posts only to `https` and follows no redirect.
- `crates/ub-fake-gmail/tests/holds.rs`: a release racing a held reply never strands it.
- `crates/ub-store/tests/store/writer.rs`, `crates/ub-store/src/writer.rs`, and `crates/ub-store/src/reply.rs`: writer priority and order, rollbacks and savepoints, panics, `rev`, stopping, awaiting results.
- `crates/ub-store/src/open.rs`: a database of another schema version, or not a database, is moved aside and a fresh one made; a locked one, or one that can't be opened, is left in place and the open fails.
- `crates/ub-core/src/tests/writer_channel.rs`: proptest that a list built from its first load and patches equals a fresh load at that `rev`; action numbers after a reload, first loads after a failed commit, `ack` never moving back and a failed batch's actions refused no later than the message that acks them, a panicking commit still going idle, searches and label reads whose commit failed (UIC-01).
- `crates/ub-core/src/subs/diff.rs`: list diffs as moves.
- `crates/ub-core/src/tests/clock.rs`: the activity tracking `settle()` relies on, a closed `Kick` waking a loop, and a stopping task group waiting for the groups inside it.
- `crates/ub-core/src/outbox/machine.rs`: every send event in every outbox state.
- `crates/ub-core/src/drafts/lifecycle/state.rs`: every draft transition in every stage: the untouched rule, waits for parts and a target's body, Gmail's ids and a create in question, a skipped Gmail change.
- `crates/ub-core/src/tests/perf.rs` (ignored; needs `UBERMENSCH_PERF_DB`): local search and autocomplete against §Performance.
- `crates/ub-bridge/src/web.rs`: the debug web bridge answers only loopback hosts and its own origins.
- `tests/layers.rs`: the crate layering in ARCHITECTURE.md.
- `tests/menu.rs`: the menu bar is Tauri's standard one with the app's own Quit in place of the predefined one, which would end the app without asking the page (UIC-05).
