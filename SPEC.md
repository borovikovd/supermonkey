# Ubermensch

A standalone macOS mail app for a real Gmail inbox. Several accounts, one window, keyboard first. The interaction model follows Superhuman Mail: a light dense inbox, a command palette, the same core shortcuts, and every action fast enough to feel instant.

Working title: Ubermensch. The user-facing name can change. The app does not use Superhuman's name, logo, or assets.

This is a local program. It installs as a normal `.app`. There is no backend server and no browser extension.

## Decisions

| Topic | Choice |
|---|---|
| Job of v1 | Personal daily driver for real Gmail |
| Platform | macOS only. Window chrome, Keychain, and shortcuts are Mac-specific |
| Look | Light, dense, single-line rows, Split Inbox, command palette, core shortcuts. See Look |
| Speed | Every local action paints within its budget. See Performance |
| Accounts | Several Gmail accounts, each with its own inbox. One account is on screen at a time, as in Superhuman |
| Must work | Read, archive, reply, send, compose, forward, palette, shortcuts, search, undo send, snooze, labels, Split Inbox, recipient autocomplete, account switching, notifications, offline reading and queued actions |
| Shell | Tauri 2.12 |
| UI | React 19.3, TypeScript 7.0.2, Vite+ 1.4 |
| Mail store | SQLite on this Mac, owned by Rust |

## Non-goals

These stay out of the product until the daily-driver loop below is solid.

- Windows and Linux
- Outlook, IMAP, or a second provider
- A unified inbox across accounts
- Dark mode
- Google Contacts sync. Autocomplete uses mail headers only
- Snippets, send later, read receipts, calendar, and learned auto-labels
- A web version
- Sharing an inbox with anyone else
- Permanent delete. Trash is enough

## Window

One window. A second launch focuses the existing window. Closing the window hides it. The app keeps running in the Dock and keeps syncing every account. Clicking the Dock icon shows the window again. `Cmd+Q` quits.

macOS title bar is overlay style: hidden title, traffic lights inset, content under the bar. The window restores its last size and position.

The window is a light canvas. A slim icon rail sits on the left. It is not a folder list. It holds icons for Inbox, Done, Snoozed, Starred, Sent, and Drafts at the top, and one avatar per account at the bottom. The conversation list fills the rest of the window. A hint bar along the bottom shows the core shortcuts.

Opening a conversation replaces the list with the conversation view. The messages sit in a centered column. `Enter` opens the highlighted row. `Esc` returns to the list and leaves the highlight. `j` and `k` inside an open conversation move to the next or previous one. Reply and forward open inside the conversation view, under the last message. A new message (`c`) replaces the screen. Neither opens a second window or a side column.

Views are reached from the keyboard and the command palette. On Inbox, the list header shows the visible splits inline (`Important 13 · Other 33`) plus search. The split holding the highlight is active, and clicking a split name scrolls the list to that split's section. Other views show their name there.

Each row is one line: the sender, the subject, the snippet, and the time. Unread rows have a dot and a heavier sender. The highlighted row is a light highlight. `j` and `k` move that highlight. In the Inbox they also cross from one split into the next. Split headers are not selectable.

Empty inbox is a quiet inbox-zero state, not a splash.

## Look

The look copies Superhuman's current light theme, the one Superhuman calls "Superhuman" in Theme Settings, not Classic. Values come from three sources:
- Superhuman's public web-app stylesheet: colors, fonts, the message column, and motion;
- pixel measurements of the app from the superhumanisdead clone spec: row height, the rail, the row highlight, the unread dot, split headers, the palette, and the hint bar;
- this spec's own picks, where neither source has a value. These are marked.

Superhuman's typefaces are its own licensed files, so this app uses the system font. All values live in one Tailwind theme file. A detail not covered here follows the same rules: quiet, dense, one accent, no decoration.

### Type

System font (`-apple-system`, SF Pro). Numbers in rows use tabular figures.

| Use | Size / line height | Weight |
|---|---|---|
| Hint bar, counts, shortcut chips | 11 / 16 | 500 |
| Timestamps | 12 / 16 | 400 |
| List rows, palette rows, buttons, compose fields | 13 / 18 | 500 read sender, 600 unread sender; 400 subject and snippet |
| Message text written by the app (plain-text mail, compose) | 15 / 26 | 400 |
| Conversation subject | 18 / 27 | 600 |
| Inbox-zero line (own pick) | 15 / 22 | 500 |

HTML mail keeps its own fonts and sizes inside its iframe.

### Color

| Token | Value | Use |
|---|---|---|
| `bg` | `#FFFFFF` | Canvas, list, conversation |
| `surface` | `#FCFAF7` | Icon rail, side panels, collapsed messages |
| `fill` | `#F2F0EB` | Selected (`x`) rows. Hover is `fill` at half strength (own pick) |
| `border` | `#DEDBD5` | Hairlines between sections, never between rows |
| `border-2` | `#EDEBE5` | Hairlines inside a message and inside sheets |
| `text` | `#141413` | Senders, subjects, body |
| `text-2` | `#73716D` | Snippets, read senders, inactive split names |
| `text-3` | `#A3A09A` (own pick) | Timestamps, hints, counts |
| `accent` | `#714CB6` | Highlight bar, selected-message bar, focus rings, primary button |
| `accent-soft` | `#F7F5FF` | Highlighted row |
| `link` | `#4198D2` | Links the app draws |
| `unread` | `#71C0EA` | Unread dot |
| `hint` | `#EDEEFA` | Hint bar |
| `danger` | `#CD0037` | Errors, invalid recipients |

The palette is dark in the light theme, as in Superhuman. Its colors are own picks: `#1F1E1D` background, `#F2F0EB` text, `#A3A09A` secondary text, `#33312E` highlighted row.

Account avatars use six fixed hues, assigned in the order accounts are added: `#714CB6`, `#D6478A`, `#DB7A2B`, `#2E9C6A`, `#2A8CC4`, `#B08A2E` (own picks). The avatar is the hue at 15% fill with the account's initial in the full hue.

### Layout

- Spacing steps of 4px.
- List rows are 36px tall with 16px side padding. There are no row dividers. Columns:
  - an unread dot, 5px in `unread`;
  - the sender, 180px (own pick);
  - the subject and snippet in the remaining width, sharing one line, separated by a thin dash, and truncated at the end;
  - the time, right-aligned.
- The highlighted row is `accent-soft` with a 3px `accent` bar on its left edge. A row that is both highlighted and selected is `fill` with the bar.
- Split names sit inline at the top of the list, for example `Important 5 · Other 1`. The split holding the highlight is in `text` at 600. The others are in `text-2`. Counts follow each name.
- Split section headers inside the list are 28px tall: name in `text-2`, count in `text-3`.
- The icon rail is 40px wide with 16px Lucide icons at 1.5 stroke.
- The hint bar is 18px tall, with hints such as `E to Mark Done · H to snooze · ⌘K for commands`.
- The conversation column is at most 768px wide (640px plus 128px of padding) and at least 520px, centered, with 32px top padding.
- The palette is 680px wide with an 8px radius, placed a third of the way down the window. It shows at most 8 rows of 36px, with shortcut chips on the right.
- Sheets are 550px wide, with 20px by 32px padding. Controls have a 6px radius, and sheets and toasts an 8px radius.
- Only the palette, sheets, and toasts have a shadow.
- Toasts sit bottom-center, above the hint bar.

### Motion

Motion never delays a result. Superhuman uses few animations, and so does this app.
- `j` and `k`, view switches, account switches, and rows leaving the list have no animation.
- The palette, sheets, and toasts fade in over 150ms ease, Superhuman's usual transition. Tooltips fade in over 200ms.
- Reduce Motion turns all of these off.

## Performance

Speed is the product. Budgets are measured from keydown to the frame that shows the result, on an M1 MacBook Air with the perf fixture. The fixture has two accounts, each with 20,000 cached threads, 2,000 of them in Inbox, one 60-message thread, and 5,000 contacts. An account switch is measured to an account not yet shown since launch.

| Action | Budget |
|---|---|
| `j` / `k`, `x`, opening the palette, each keystroke in the palette | 50 ms |
| Autocomplete suggestions for each keystroke | 50 ms |
| `e`, `#`, `!`, `s`, `u`, `z`, a snooze or label choice, view switch, account switch | 100 ms |
| Opening a cached conversation, up to the first painted message | 100 ms |
| Opening the reply composer in an open conversation, ready to type | 100 ms |
| `r` or `f` on the list: conversation and composer, ready to type | 150 ms |
| Local search results for each keystroke | 100 ms |
| A keystroke in compose | one frame (16 ms) |
| Scrolling the list or a conversation | 60 fps, no blank rows |
| Cold launch to a usable cached Inbox | 1.5 s |

To meet them:
- The network is never on the path of a budgeted action.
- A mail action updates the mirror in the same frame as the keypress. Rust then writes SQLite and queues the Gmail call. See UI state.
- The highlighted or open conversation and its neighbors are subscribed once the highlight rests, so opening one paints from the mirror.
- Bodies are sanitized at ingest, not at open.
- Collapsed messages create no iframe.

A debug build shows keydown-to-paint for each shortcut and logs every action over budget. The perf fixture is generated by a script, and the budgets are checked before each build step is called done.

## Views

Views show the account on screen. Each account has its own views, splits, pins, undo history, last view, and highlight.

A thread's latest message is its newest message that is not a draft and not in trash or spam. If no message qualifies, it is the newest non-draft message, or the draft when that is all the thread has. The row's sender, snippet, and sort time come from it.

A thread's views come from its messages' labels. It is in Trash or Spam when any of its messages has `TRASH` or `SPAM`. It is in Inbox, Starred, Sent, or a label view when a message that is not in trash or spam has that label. Trashing one message therefore leaves the thread in the views its other messages put it in.

| View | What it lists |
|---|---|
| Inbox | Threads with `INBOX` by the rule above, not snoozed, grouped into splits |
| Done | Threads marked `seen_in_inbox` that are no longer in Inbox and not snoozed, and that have at least one message that is not a draft and not in trash or spam. `seen_in_inbox` is set when this app sees the thread with `INBOX` while the account is connected, and is never cleared |
| Snoozed | Threads with `snooze_until` set, whether or not it has passed |
| Starred | Gmail `STARRED` by the rule above |
| Sent | Gmail `SENT` by the rule above |
| Drafts | Gmail drafts plus local drafts not yet pushed, except drafts with an outbox row |
| Label | Threads with that label, by the rule above |
| Search | Local matches, then Gmail matches while online |
| Trash, Spam | Threads in Trash or Spam by the rule above |

Most labeled, trashed, and spam mail is older than the first sync. Opening a Label, Trash, or Spam view while online therefore also runs `threads.list` with that label's id, and merges the results the way Search does. Trash and Spam set `includeSpamTrash=true`.

`g` then a letter jumps to a view: `e` done, `h` snoozed, `s` starred, `t` sent, `d` drafts, `l` label picker, `#` trash, `!` spam. From any view, `g` then `i` and `g` then `o` open Inbox on the Important and Other splits. The header stays `Inbox`. If that split is hidden because it is empty, the jump shows its header. If Important is turned off, `g` then `i` opens Inbox and does not show that split.

## Accounts

Add an account from the first-run screen or the palette (`Add account`). Sign-in is Google OAuth for an installed app, in the system browser only. It uses a loopback redirect to `127.0.0.1` on a free port, PKCE, and the scope `https://www.googleapis.com/auth/gmail.modify`. That scope covers read, send, drafts, labels, archive, and trash. It does not allow permanent deletion of threads and messages. Deleting a draft is allowed. Google blocks sign-in from embedded webviews, so the app never uses one.

The OAuth client is a Desktop-type client in the user's own Google Cloud project. Its client ID and secret are build-time config. Google treats a desktop secret as not confidential. The consent screen is published In production, not Testing, because Testing expires refresh tokens after 7 days. The app stays unverified, so each sign-in shows Google's unverified-app warning.

The refresh token goes into the macOS Keychain. Service name `ubermensch`, account name the email address. Access tokens stay in memory.

If Google rejects the refresh token (`invalid_grant`), the account shows Sign in again. Its cached mail stays readable. Its queue and outbox wait until sign-in succeeds.

Removing an account deletes its Keychain item and its rows from the local database. If it was on screen, the next account in the rail takes its place.

### Switching

One account is on screen at a time. `Ctrl+1` to `Ctrl+9` switch to the accounts in rail order, as do clicking an avatar and the palette's `Switch account`. Each avatar shows that account's unread Inbox count, hidden at zero. Switching:
- closes the palette and search
- closes an open composer as a second `Esc` would: kept as a draft in its own account, or discarded if it is an untouched fresh composer
- returns to the new account's last view and highlight, or its Inbox with the top row highlighted on the first visit since launch. An open conversation is not reopened.

Send toasts stay on screen across switches. All accounts keep syncing, notifying, and running their queues and outboxes while another account is on screen.

Reply, reply all, and forward send from the account on screen, which owns the thread. A new message sends from the account on screen.

## Keyboard

The shortcut table applies to the list and the conversation view. While compose, search, or the palette is focused, keystrokes go to that field, including keys held with Shift, and the shortcut table does not apply.

In compose, `Tab` and `Shift+Tab` move between fields, unless the autocomplete list is open. `Cmd+K` inserts a link, and `Cmd+Enter` sends. The first `Esc` blurs the editor and leaves compose on screen. With compose blurred:
- `Cmd+K` opens the palette with draft actions (send, attach, discard)
- `Enter` refocuses the editor
- `Cmd+Enter` sends
- the compose shortcuts in Compose work
- a second `Esc` closes compose and keeps the draft
- other keys do nothing

A composer started fresh by `r`, `Enter`, `f`, or `c` is discarded on close, locally and on Gmail, when it is untouched. Untouched means:
- subject, body, and attachments are as the composer started them
- recipients and From are the defaults

A stray `r` leaves nothing behind. A resumed draft, or one reopened by undo, is never discarded this way.

`Esc` in the palette closes it. Search has its own `Esc` rule below.

The palette lists actions and views. Each row shows its shortcut on the right. `?` opens the full shortcut sheet.

| Key | Action |
|---|---|
| `j` / `k` | Next / previous conversation, across split boundaries |
| `Tab` / `Shift+Tab` | Next / previous split. In the conversation view, cycle links. In compose, move fields |
| `Enter` | Open the conversation. Inside it, reply all. On a link reached with `Tab`, open the link |
| `e` | Mark done (archive) |
| `Shift+E` | Return to inbox, and clear a snooze |
| `h` | Snooze |
| `c` | Compose |
| `r` | Reply |
| `f` | Forward |
| `Cmd+K` | Command palette, or insert link in compose |
| `/` | Search |
| `z` | Undo |
| `s` | Star or unstar |
| `u` | Mark read or unread |
| `#` | Trash |
| `!` | Spam |
| `l` | Add or remove labels |
| `y` | Remove the current view's label, when that label can be removed |
| `n` / `p` | In a conversation, select the next / previous message |
| `o` | Expand or collapse the selected message |
| `Shift+O` | Expand every message and all quoted history. Again to restore |
| `Ctrl+1`…`Ctrl+9` | Switch account |
| `Cmd+Enter` | Send |
| `Esc` | Clear a link reached with `Tab`. Otherwise close the open conversation, palette, search, snooze picker, or compose. On the list, clear the selection |
| `?` | Shortcut sheet |

`x` selects a conversation, and `Shift+J` / `Shift+K` extend the selection. With a selection, `e`, `Shift+E`, `h`, `s`, `u`, `#`, `!`, `l`, and `y` apply to every selected conversation. `s` and `u` follow the `l` rule: if every selected conversation is already starred (or unread), they all lose it. Otherwise they all get it. Plain `j` and `k` replace that selection with the one conversation they move to. `Esc` clears a selection only when the palette, search, snooze picker, and compose are all closed. Otherwise it only closes the open one.

When the acted-on conversation leaves the current list:
- On the list, the highlight moves to the next row. If it was the last row, it moves to the previous row.
- In an open conversation, the next conversation opens, or the previous one if it was last.
- If none remain, the app returns to the empty list.

When a patch from Rust removes the highlighted row without the user acting on it, for example after a poll, the highlight moves the same way. An open conversation whose row leaves that way stays open, and `j` and `k` move from its old position.

### Undo

Every action in the table that changes mail or splits is one undo entry, however many conversations it touched. Sends and pins are entries too. Marking read on open is not. Each account has its own undo history. A send entry points at its outbox row. A pin entry stores the pin it replaced. A mail entry stores the labels and `snooze_until` values the action changed, with their values from before it. Undo reverts only those, so later changes such as read-on-open or edits from another Gmail client stay. So undoing a trash does not bring back a reply whose send was already undone, and undoing `e` on a snoozed thread restores its snooze. Once a send is sent and its toast is gone, its entry leaves the undo history. Closing a send toast is an action that tells Rust, and on launch Rust drops send entries whose row is already sent. A stray `z` later never trashes delivered mail. `z` undoes the newest entry of the account on screen. Pressing it again undoes the one before. A send toast's Undo is the exception: it always undoes its own send and removes that entry, whatever is newer. The last 50 entries per account are kept in SQLite, so `z` survives a restart.

## Reading

Opening a thread marks it read locally and on Gmail. That read change is not an undo entry.

The conversation view lists messages oldest first.
- The latest message and every unread message are expanded.
- Other messages are collapsed to one line: sender, snippet, and time.
- When more than four collapsed messages sit together, the ones between the first and the last fold into a single `N older messages` line.
- Clicking a collapsed line or a fold expands it. Clicking an expanded message's header collapses it.
- The view opens scrolled to the first unread message, or to the latest message when none are unread.

Draft messages are not listed with the others. Outside Trash and Spam, messages in trash or spam are not listed either, so an undone sent reply disappears from the thread. In Trash and Spam, every non-draft message is listed. A reply or forward draft on the thread shows as a collapsed draft below the last message. Clicking it resumes it in the reply composer. `r`, `Enter`, and `f` resume the newest draft of their kind aimed at the selected message (see Reply composer).

In Drafts, `Enter` on a new-message draft opens it on the `c` screen. On a reply or forward draft, it opens the thread with that draft in the reply composer.

One message is selected, marked by an accent bar on its left. It starts as the latest message, even when the view opens scrolled to an earlier unread one, so `r` from the list and `r` after opening reach the same person. Collapsed drafts and the Sending row are never selected. `n` and `p` select the next and previous message and scroll it into view. Selecting a message inside a fold, by any path, expands the fold. Clicking a message selects it. `o` expands or collapses the selected message. `r`, `Enter`, and `f` act on the selected message.

When a new latest message arrives while the selection is on the previous latest message, the selection moves to the new one. When the selected message leaves the conversation, the latest message is selected. The selection belongs to the open conversation and resets to the latest message whenever a conversation opens, including the same one again.

Quoted history is collapsed behind a `•••` toggle that expands it on click. `Shift+O` expands every message and all quoted history. Pressing it again returns to the default.

HTML is sanitized in Rust with ammonia before it is stored. The sanitizer keeps `style`, `class`, and `id` attributes, `<style>` blocks, and table and layout attributes, and allows `data:` image URLs. It drops scripts, event handlers, forms, and `javascript:` URLs. Inline images referenced by `cid:` are fetched with the body and rewritten to `data:` URIs before sanitizing.

Each expanded message renders in a `srcdoc` iframe with `sandbox="allow-same-origin allow-scripts"`. Its document starts with `<meta http-equiv="Content-Security-Policy" content="script-src 'none'">`, so nothing in the message can run. `allow-scripts` is there only because WebKit does not fire the parent's listeners inside a sandboxed iframe without it (WebKit bug 218086). The parent can reach the iframe's document. The parent:
- sizes each iframe to its content, so the conversation scrolls as one page
- forwards keystrokes from the iframe to the shortcut handler
- cycles the iframe's links for `Tab`
- catches link clicks and opens them in the default browser through the opener plugin

Links never navigate the app window.

Remote images load by default. A setting turns them off. With remote images off, the iframe's CSP also sets `img-src data:`, which blocks remote images in CSS too. Tracking pixels are a consequence of the default.

Attachments download on demand into the app cache and open with the default app. They are not fetched during the initial sync.

## Compose

TipTap edits the draft: bold, italic, underline, strike, lists, and links. These shortcuts work in any composer, focused or blurred, as in Superhuman:

| Key | Action |
|---|---|
| `Cmd+Shift+O` | Focus To |
| `Cmd+Shift+C` / `Cmd+Shift+B` | Show and focus Cc / Bcc |
| `Cmd+Shift+F` | Open the From picker |
| `Cmd+Shift+S` | Focus Subject |
| `Cmd+Shift+M` | Focus the message body |
| `Cmd+Shift+A` | Attach a file |
| `Cmd+Shift+,` | Discard the draft |
| `Cmd+Enter` | Send |
| `Cmd+B` / `Cmd+I` / `Cmd+U` | Bold / italic / underline |
| `Cmd+Shift+X` | Strikethrough |
| `Cmd+Shift+7` / `Cmd+Shift+8` | Numbered / bulleted list |
| `Cmd+K` | Insert a link while the editor is focused |

TipTap's own shortcuts that clash with this table are removed: `Cmd+Shift+B` (blockquote), `Cmd+Shift+S` (strike), and `Cmd+Enter` (hard break). Strike is `Cmd+Shift+X` only, and a hard break is `Shift+Enter` only.

### Reply composer

In a conversation, `r`, `Enter`, and `f` open the reply composer inline, under the last message, aimed at the selected message. `r` and `f` pressed on the list open the conversation first and aim at its latest message. `Enter` on the list only opens the conversation. The thread stays above and scrolls with the mouse or trackpad. The view scrolls so the composer is fully visible, and the editor takes focus.

A forward stays on the thread, like a reply, with thread id, `In-Reply-To`, and `References` set. Drafts on a thread come in two kinds: reply (from `r` or `Enter`) and forward (from `f`). `r` and `Enter` resume the newest reply draft aimed at the selected message, and `f` the newest forward draft aimed at it. With none aimed at the selected message, and the selection on the latest message, they resume the newest draft of their kind that was started on the thread's latest message, so a half-written reply survives new replies. A draft started on an earlier message resumes only from that message, or by clicking it. A draft from Gmail counts as started on the latest message when its target was the newest non-draft message older than the draft. Otherwise they start a new one aimed at the selected message. A draft from Gmail is aimed at the message its `In-Reply-To` names, or at the latest message when none matches. Whenever a reply composer opens, by any path, its target message is selected. A draft this app made keeps the kind it was started with. A draft that first appears from Gmail gets its kind once, at that sync, and keeps it:
- new, when it is alone in its thread and has no `In-Reply-To`
- forward, when its subject starts with `Fwd:`
- reply, otherwise

A thread can hold several drafts, for example from Gmail web or after an undo. Each shows as a collapsed draft. A draft in the outbox shows only as the Sending message and is never resumed.

Closing the reply composer leaves the conversation open. Sending closes the composer and keeps the conversation open. The pending message shows at the bottom of the thread, marked Sending, until Gmail accepts it. The conversation shows an outbox row until the thread holds the message with that row's sent message id. From then on it shows only the message.

`c` opens a new message that replaces the screen. Closing or sending it returns to the screen it came from.

### Addressing

Reply and reply all address the target message, the one the composer was aimed at, and quote it as a collapsed block under the editor. After forwarding Alice's mail to Bob, the forward is the latest message, so `r` answers Bob. `p` to Alice's message, then `r`, answers Alice.

Reply and reply all send from the `sendAs` address the message was sent to, or the default `sendAs` address when none matches. If this account sent the message, they send from that message's From.

The quote is not part of the TipTap document. The composer shows it as a collapsed, read-only block, and Rust builds it from the stored target message when it builds the MIME. Rust wraps its quote in `<div class="gmail_quote">`, as Gmail does. A reply's block starts with an `On <date>, <name> wrote:` line. A forward's block starts with `---------- Forwarded message ---------` and the original From, Date, Subject, and To lines. For a draft read from Gmail, everything from the `gmail_quote` element onward becomes that block, and only the text above it goes into TipTap. This applies to Gmail-made drafts and to this app's own drafts read back, for example after re-adding an account.

When a draft read from Gmail is first opened in a composer, Rust downloads its attachments and inline parts before its first upsert. Offline, the composer still opens, and the downloads run before the queued upsert or the send. `drafts.update` replaces the whole message, so every upsert carries all of the draft's attachments. Forward and new messages send from the default. The From field shows the chosen `sendAs` address, and its picker lists the account's `sendAs` addresses.

Gmail does not show `data:` images. When building the MIME, Rust turns images that came from `cid:` parts, in quotes and forwards, back into `cid:` inline parts, using the original parts. The sent message stays on that thread: same thread id, with `In-Reply-To` and `References` set. Reply sets To to that message's Reply-To, or From if it has no Reply-To. Reply all sets To the same way, then puts the message's other To and Cc on Cc, leaving out this account's addresses. If this account sent the message, Reply and reply all set To from that message's To instead, and reply all still puts the remaining recipients on Cc. Forward quotes the target message, carries its attachments, and starts with an empty To. Attachments not yet cached download when the send is built, so a forward works offline.

### Recipients and autocomplete

To, Cc, and Bcc hold recipients as chips. Cc and Bcc are hidden until used, or until `Cmd+Shift+C` or `Cmd+Shift+B` shows them.

As you type, a list of up to six suggestions opens under the field. It shows name and address, with the matched part in bold. While it is open:
- `↓` and `↑` move through it
- `Enter` or `Tab` accepts the highlighted suggestion
- `Esc` closes the list without leaving the field

Typing a comma, or a space after a complete address, turns the typed text into a chip. So do `Tab` and `Enter` with no list open, leaving the field, and Send. `Backspace` in an empty field selects the last chip, and a second `Backspace` removes it. Pasting a list of addresses makes one chip each. A chip that is not a valid address shows in `danger`, and Send refuses with a toast until it is fixed.

Suggestions come from the account's own mail, in a local `contacts` table. Every synced message adds its From, To, and Cc addresses. Drafts and spam add nothing.
- On mail this account sent, each To and Cc address gains a sent count.
- On mail it did not send, only From gains a received count. The other To and Cc addresses are stored with no count, unless the message is list mail, which adds nothing for them. Each message id counts once, so a resync or the backfill never counts a message twice. Each address keeps:
- its latest display name
- how many messages this account sent to it
- how many it received from it
- when it was last in a message

A query matches the start of any word in the name, the start of the address, or the start of the domain. Ranking:
- addresses this account has sent to come first, ordered by sent count, weighted toward recent contact
- then addresses it has only received from
- then addresses seen only as co-recipients
- never suggested: an address this account has never sent to that has appeared only on list mail
- never suggested: addresses with `noreply` or `no-reply` in the local part
- the account's own addresses are never suggested

Autocomplete works offline. After the first sync, Rust backfills contacts in the background from the To, Cc, and Date headers (`format=metadata`) of the last year of sent mail.

### Drafts

Drafts autosave locally. They also upsert to Gmail drafts, through the queue when offline. Rust stores the message id and thread id each upsert returns. When sync finds that a draft's Gmail message changed to one this app did not write, the draft is read again from Gmail, unless a composer has it open. A change skipped because a composer had the draft open is read when that composer closes without edits. A draft deleted on Gmail, for example because it was sent from a phone, is deleted locally. If a composer has it open, the local draft is kept with its Gmail id cleared, the composer stays, and a toast says `Draft was sent or deleted elsewhere`. Its next upsert uses `drafts.create`. Send cancels any queued upsert for that draft, because the outbox writes the final version itself. Discard deletes the local draft and the Gmail draft.

Attachments can be added from the compose header. The Rust side builds the MIME message with mail-builder, including encoded headers and attachments. mail-builder's `inline()` puts inline parts in `multipart/mixed`, so Rust builds the tree itself with `MimePart` and passes it to `.body()`: `multipart/mixed` holding `multipart/related` (the `multipart/alternative` body, then the inline parts), then the attachments. With no attachments, `multipart/related` is the top part.

## Send and undo

Send does not call Gmail immediately. The draft goes into a local outbox. The row starts as waiting, with `send_after` ten seconds ahead. A toast shows Sending and Undo. The toast's Undo always acts on its own send (see Undo). `z` acts on a send only while that send is the newest undo entry of the account on screen.

When `send_after` passes, the row becomes sending. Rust then:
1. Downloads any of the draft's attachments and inline parts not yet cached: forwarded ones, and those of a draft read from Gmail.
2. Builds the MIME message.
3. Writes it to the Gmail draft with `drafts.create` or `drafts.update`.
4. Calls `drafts.send`.

Gmail removes the draft as it sends. When Gmail accepts, the row becomes sent and stores the sent message id. Rust then fetches that message and stores it at once, so the reply never leaves the thread while waiting for the next poll.

Undo on a send:
- Waiting: cancel the send and reopen the composer with the draft. This also works after a restart, and after the timer has passed while Gmail is unreachable.
- Sending: the undo commits at once, its entry leaves the history, and Rust marks the row. When the send resolves, Rust trashes the sent message, or, if it was not sent, puts the draft back in a composer through a notice.
- Sent: trash only that sent message. Other messages keep their labels, and the thread stays in every view they put it in.

If Gmail cannot be reached before `drafts.send` is called, the row goes back to waiting and retries when the app is online. If `drafts.send` gets no clear answer, the row stays sending, and an Undo on it is held as above. When online, Rust calls `drafts.get`. A 404 means the draft was sent, so the row is sent, and its message id is the newest `SENT` message from this account on the thread. If the draft still exists, Rust calls `drafts.send` again. If Gmail rejects the message, a toast shows the error.

Undo on a waiting send, a held undo that resolves as not sent, or a rejection puts the draft back in a composer:
- The app switches to the send's account if needed.
- A reply or forward reopens its thread with the reply composer, even if the thread has left the current view. A new message reopens the `c` screen.
- If another composer is open, it is first closed as a second `Esc` would close it.
- A rejection, or a held undo that resolves as not sent, that arrives while the user is typing in a composer does not take over. Its toast offers `Open draft` instead.
- For Undo on a waiting send, the composer opens once the channel message whose `ack` covers the undo shows the outbox row gone. If Rust held the undo because the row was already sending, no composer opens until its notice arrives.

`Cmd+Q` does not send waiting rows early. They stay in the outbox and send on the next launch, once online, as Superhuman does. A send that misses its time, offline or across a quit, shows `Email delayed` until Gmail accepts it. A sending row with no answer at quit is settled by `drafts.get` on the next launch.

The delay is a fixed ten seconds, as in Superhuman, which does not let users change it.

## Archive, trash, star, read

Archive removes the `INBOX` label. On a snoozed thread it also clears `snooze_until`. The thread stays in All Mail and appears in Done. A later reply from someone else puts it back in the inbox, which is ordinary Gmail behavior.

Trash adds `TRASH` and removes `INBOX` and `SPAM`. Spam adds `SPAM` and removes `INBOX` and `TRASH`. Star toggles `STARRED`. Read state toggles `UNREAD`. Return to inbox adds `INBOX`.

Archive, trash, spam, snooze, and a snooze wake act on the thread's non-draft messages at the time of the action, through `messages.batchModify` with those ids. Gmail does not label drafts, and a draft's message id changes on every update. The op stores the ids, so a delayed replay or retry never touches messages that arrived later. A reply that arrives while an archive is queued still lands in the inbox.

Every one of these updates the list in the same frame, writes SQLite, then calls Gmail. Label changes and snooze do the same. If Gmail cannot be reached, the local change stays and the action queues. If Gmail rejects it, as defined in Gmail errors, the row rolls back and a toast shows the error.

## Labels

Labels come from Gmail, per account. System labels map onto the views above. User labels appear in the `l` picker and in `Go to label`.

`l` opens a palette picker of the account's user labels. If every selected thread has the chosen label, it is removed from all of them. Otherwise it is added to all of them. The picker can create a label. If `labels.create` reports that the name already exists, Rust refreshes `labels.list` and uses that label. This is not a rejection.

`y` removes the current view's label when that label is a user label, `INBOX`, `STARRED`, `TRASH`, or `SPAM`. It does nothing in Sent, Drafts, Done, Snoozed, and Search.

Label colors and nested-label editing wait. Nested labels display as the path Gmail already returns.

## Split Inbox

Each account's Inbox is one list divided into sections. A conversation appears in exactly one section. Custom splits come first, then Important, then Other. Other is the catch-all, always last, and cannot be deleted.

Each account starts with Important and Other only. The palette command `Split Inbox settings` opens a sheet for the account on screen. The sheet can add, rename, reorder, turn off, and delete custom splits. Important can be turned off and cannot be deleted. Other cannot be deleted. Turning a split off keeps its definition and skips it. Deleting a custom split drops its rules and pins, and those conversations fall through.

Each section header in the list and each split name in the list header shows the split name and the conversation count. The count is hidden at zero and caps at `999+`. A split can hide itself while empty, which hides both its name in the list header and its section. That option defaults on.

With the list focused, `Tab` selects the first conversation of the next visible split and scrolls its header into view. `Shift+Tab` goes to the previous one. `Tab` on the last split and `Shift+Tab` on the first do nothing. `g` then `i` and `g` then `o` do that move inside Inbox.

### Where a conversation lands

Rust assigns the split locally whenever a thread changes, and stores it on the thread. The list never waits on the network.

Rules and pins read the thread's latest message from someone else. That is the newest message that is not a draft, not in trash or spam, and not sent from this account. When every message is from this account, they read the latest message. Your own reply does not move a thread.

First match wins:

1. A pin whose split is still enabled. A sender pin beats a domain pin, which beats a label pin. When several label pins match, the split that comes first in split order wins. A label pin matches the thread's user labels, by the rule in Views, because Gmail does not copy a thread's user labels onto later messages. Turning a split off keeps its pins and skips them.
2. Enabled custom splits, top to bottom.
3. Important, when it is enabled and the message looks person-to-person.
4. Other.

Person-to-person means the message is not list mail, and that message has `CATEGORY_PERSONAL` or no category label. List mail is anything with a `List-Id` or `List-Unsubscribe` header, stored as a flag at ingest. `CATEGORY_PROMOTIONS`, `CATEGORY_SOCIAL`, `CATEGORY_UPDATES`, and `CATEGORY_FORUMS` on that same message are not Important. Each message stores its own label ids.

`Cmd+K` → `Add to split` pins the message's sender, that sender's domain, or one of the thread's user labels to a split you pick. `Move to Important` and `Move to Other` offer that same choice and pin it to Important or Other. A new pin replaces any pin of the same kind and value, so one sender or domain or label is pinned to at most one split. `z` removes the new pin and restores the one it replaced.

### Custom rules

A custom split is a name plus a query. The query matches the message the rules read. The supported language is `from:`, `to:`, `cc:`, `bcc:`, `subject:`, `label:`, `is:list`, bare words, `AND`, `OR`, and parentheses.
- A bare word matches the sender or the subject.
- Terms written next to each other are AND. AND binds tighter than OR.
- `from:`, `to:`, `cc:`, and `bcc:` match the display name or the email address.
- `label:` with a user label name matches the thread's user labels, ignoring case.
- `label:` with a system or `CATEGORY_` id matches that message's own labels.

The settings sheet offers these starters, off until turned on:

| Starter | Query |
|---|---|
| Newsletters | `is:list` |
| Promotions | `label:CATEGORY_PROMOTIONS` |
| Social | `label:CATEGORY_SOCIAL` |
| Updates | `label:CATEGORY_UPDATES` |

Turning every custom split off, and turning Important off, leaves a flat inbox whose only section is Other.

## Snooze

`h` opens a picker: later today, tomorrow morning, next week, and a date-and-time choice.
- Later today is 18:00 local. It is omitted after that time.
- Tomorrow morning is 08:00 local on the next calendar day.
- Next week is 08:00 local on the coming Monday, or on the Monday after when today is Monday.

Snooze removes `INBOX` on Gmail and stores `snooze_until` locally, so the thread leaves the inbox on other Gmail clients too. Only this app wakes it. While the app is not running, a due thread stays out of every Gmail inbox until the next launch.

When `snooze_until` is reached, the app adds `INBOX` and clears `snooze_until` locally at once. Every due snooze is also woken the same way:
- on launch, before the list is shown
- after the Mac wakes

If Gmail cannot be reached, that label update stays queued while the thread is already back in the local inbox. A due snooze that has not been woken yet, because the app was closed or the Mac was asleep, stays in Snoozed and out of Done.

A snoozed thread that gains `INBOX` from outside this app loses its snooze and returns to the inbox. That covers a new message that arrives with `INBOX`, including mail from your other accounts, and a move to inbox on another Gmail client. Your own reply does not add `INBOX`, so it does not clear the snooze. Archive, trash, and spam also clear a snooze.

History also returns this app's own label changes. So the rule is judged on the thread's net labels after a whole history batch is applied, once, after the batch's last transaction, skipping messages with an op still pending. A single history record never clears a snooze. Undoing a snooze and snoozing again before the next poll leaves the new snooze in place.

Snoozed threads live in the Snoozed view. `z` undoes a snooze by restoring the thread's state from before it: back in the inbox, or the earlier snooze time when it was re-snoozed. `Shift+E` does the same when the thread is snoozed: it clears `snooze_until` and adds `INBOX`. From Trash or Spam, return to inbox also removes `TRASH` or `SPAM`.

## Settings

The palette command `Settings` opens the Settings sheet, as in Superhuman. `Cmd+,` opens it too, as in other Mac apps. It has:

- Remote images: on or off. Default on.
- One block per account: its address, `Mute notifications`, `Split Inbox settings`, `Sign in again` when the account needs it, and `Remove account`.

Changing a setting is an action, as in UI state. It is not an undo entry. Settings are stored in the `settings` table and apply at once.

`Remove account` asks to confirm. If the account's queue or outbox is not empty, the confirmation says how many actions and sends will be lost. Removing an account:
- drops its queue and outbox, and stops its sync
- deletes its rows from every table and its downloaded attachments
- deletes its tokens from Keychain and revokes the refresh token with Google when online
- if it was on screen, switches to the next account on the rail, or shows the first-run screen if none are left

## Search

`/` focuses search for the account on screen. Results replace the list. `Enter` or `↓` moves focus from the field to the results, where the shortcut table applies. `Esc` in the field or on the results restores the previous view.

The first results come from local full-text search, and they appear immediately, including offline. Like Gmail search, local search skips threads with no message outside trash and spam. The local query language is the custom-rule language. A bare word matches the sender, the subject, or the body text. Local search ignores operators it does not support. Gmail applies the full query.

While online, the app also runs the query through Gmail `threads.list` and merges anything the local index does not have yet. Gmail gets the query without `is:list`, which it does not know. A hit from that list has only ids. The row appears once a metadata fetch has the sender, subject, snippet, and time. The body fills in after that.

The Gmail query runs once typing pauses for 300 ms, and a newer query cancels the older one's queued fetches. Gmail search and the `threads.list` of Label, Trash, and Spam views ask for 15 threads at a time and fetch the next page when the list nears its oldest Gmail hit, not its end. A page costs about 610 units, so it fits in the quota bucket in Sync. A hit's body is fetched at priority 5, or at once when it is opened. A hit already cached needs no fetch.

Offline, the search field notes that results are from this Mac.

## Offline

The cached mailbox stays readable with no network: list, thread, search of cached mail, and recipient autocomplete.

These actions queue and replay in order per account when the network returns: archive, trash, spam, star, read-state, return to inbox, label changes, label create, snooze, unsnooze, draft upsert, and discard. A label created offline gets a local id, and replay maps it to the Gmail id. Sends wait in the outbox instead.

The queue is SQLite, not memory. A restart does not drop it. A queued action applies to the messages it recorded, so it wins over a conflicting remote change to those messages without hiding newer mail. After replay, a history sync reconciles. Loss of network leaves the queued action in place. A rejection from Gmail, as defined in Gmail errors, rolls that action back, toasts the error, and replay continues with the next action.

## Notifications

A message that `history.list` reports as added with `INBOX` and `UNREAD` raises a macOS notification, for any account, whichever is on screen. The notification names the account. There are two exceptions:
- its From address is one of the connected accounts
- the focused window is already showing that thread

The connect sync and a full resync do not notify. More than three new messages for one account in one poll become one summary notification.

Clicking the notification shows the window, switches to that account, opens Inbox, and selects the thread. A summary switches to that account and opens Inbox.

The Dock badge is the sum of unread Inbox threads across accounts that are not muted.

Notifications are on for each account until that account is muted. The palette's `Mute notifications` and `Unmute notifications` act on the account on screen. A muted account still shows its count on its avatar but does not add to the Dock badge.

## Sync

Rust is the only thing that talks to Gmail. The UI reads SQLite.

On connect, store the account profile and `historyId`. The display name comes from the account's primary `sendAs` entry. Store every `sendAs` address too, and refresh them on focus. Wherever the spec says "this account's addresses", "sent from this account", or "this account sent", it means any of those addresses. Then fetch:

- all labels
- every inbox thread, metadata then body
- every draft, listed with `drafts.list`, which gives the draft, message, and thread ids, metadata then body
- sent mail from the last 90 days, metadata then body
- starred threads, metadata then body
- in the background, the contacts backfill (see Compose)

Older mail arrives when search or an open hits it.

Each list is fetched newest first. Metadata uses `threads.get` with `format=metadata`, and bodies use `threads.get` with `format=full`. Gmail requests take this priority, highest first:
1. UI actions, sends, and draft upserts
2. A thread the user opens whose bodies are not stored yet, then its prefetched neighbors. Search, the `threads.list` of Label, Trash, and Spam views and the metadata of their hits, attachment downloads, and draft reconciliation reads rank here too
3. `history.list` polls, and the metadata fetches for the messages and threads they report
4. Bodies of new messages from a poll or a full resync, then inbox metadata, including a full resync's thread list and its refetch of Inbox threads, then drafts and starred metadata, then inbox bodies
5. Bodies of Gmail search hits and of hits from the `threads.list` of Label, Trash, and Spam views. Sent, the bodies of drafts and starred threads, and a full resync's refetch of other threads
6. The contacts backfill

With the quota below, a 2,000-thread inbox has all its rows in about 20 minutes and all its bodies about an hour after that. The newest rows show within seconds, and an opened thread fetches its bodies at once.

Polling `users.history.list` from the stored cursor starts as soon as the connect `historyId` is stored, and runs alongside the fetches above at its priority, for every account. It runs every 45 seconds while the app runs, on focus, and on wake from sleep. This app polls Gmail for changes.

If `history.list` returns 404 because the cursor is too old, run a full resync:
1. Store a fresh `historyId` from `getProfile`, so polling resumes at once.
2. List every thread id with its `historyId` through `threads.list` with `includeSpamTrash=true` and `maxResults=500`, Inbox first. This costs about 10 units per 500 threads.
3. A cached thread missing from the list is dropped only if its row has not changed since step 1 and `threads.get` (`format=minimal`) returns 404. The list is not a snapshot: a thread that changes while it is being read can move to a page already read.
4. Refetch metadata, which replaces labels, for cached threads whose `historyId` changed. Of the threads not cached, fetch only those the connect sync would fetch, listed by label as on connect. Other threads wait for search or an open, as always. Cached Inbox threads and new Inbox threads come first. Stored bodies are never fetched again. New messages get metadata and bodies as a poll's new messages do.
5. List drafts with `drafts.list`. Local drafts whose Gmail draft is gone follow the rules in Drafts. Drafts the app does not know, or whose message id changed, are read again, also by those rules.
6. Apply the snooze rule: a snoozed thread that now has `INBOX` loses its snooze.

A full resync does not notify.

Refresh `labels.list` on focus and whenever history names a label id the app does not know.

At most eight Gmail requests run at once per account. Requests for the account on screen go first. A visible sync error leaves cached mail usable.

Gmail allows each user 6,000 quota units per minute in a Cloud project created on or after May 1, 2026. Each method has a published cost: `threads.get` is 40, or 120 with `format=full`; `messages.get` and `drafts.get` are 20, or 60 with `full` or `raw`; `drafts.send` is 100. Rust spends at most about 5,800 units in any minute per account, by those costs, in the priority order above. The budget is a bucket that holds at most 1,000 units and refills at 4,800 per minute, so no minute goes over about 5,800. Priorities 4 to 6 spend at most 4,000 per minute, and only while the bucket holds more than 500 units. So an open, a search page, an action, or a poll starts at once. The budget is build-time config, because a project that used the Gmail API between November 2025 and April 2026 keeps its older, higher quota. One user at full rate stays far below the 80 million units a day at which Google plans to start billing a project.

### Gmail errors

Every Gmail call ends in one of these outcomes. Of these, only a rejection rolls back a queued action or shows an error toast. The notice for a held send undo and the `Draft was sent or deleted elsewhere` toast follow their own rules.

| Outcome | What Rust does |
|---|---|
| No connection, or the request failed before it was sent | The account is offline. The action stays queued and replays when the network returns |
| Sent, but no response (timeout or dropped connection) | Retry, except where the spec says otherwise: `drafts.send` follows Send and undo, and `drafts.create` follows the rule below |
| `drafts.send` or `drafts.create` gets a 5xx | Gmail may have done the write. For `drafts.send`, a 5xx is no clear answer and a 404 means sent, as in Send and undo. For `drafts.create`, a 5xx follows the rule below |
| 401 | Refresh the access token once and retry. If the refresh gets `invalid_grant`, the account shows Sign in again |
| 429, 5xx, or 403 with reason `rateLimitExceeded`, `userRateLimitExceeded`, or `backendError` | Back off and retry: 1 s doubling to 60 s, with jitter, or `Retry-After` when given. The action stays pending, the row stays as the user left it, and the account shows a quiet sync status after 30 s of retrying |
| 404 on a message, thread, or label an action names | It was deleted elsewhere. Rust drops it from the action. If nothing is left, the action is done. `messages.batchModify` does not say which id is missing, so Rust checks the ids with `messages.get` (`format=minimal`), drops the missing ones, and retries with the rest. Drafts follow the draft rules |
| 404 on a sync, search, or body fetch | The item is gone, for example a draft version Gmail web already replaced. Rust drops it, with no toast. A 404 from `history.list` runs the full resync in Sync |
| 409 on `labels.create` | The label exists. Use it, as Labels says |
| Any other 4xx | A rejection |

Access tokens refresh shortly before they expire, so a 401 is rare.

`drafts.create` is the only write besides send that is unsafe to repeat. Rust puts an `X-Ubermensch-Draft` header with the local draft id in every draft it writes. After a `drafts.create` with no clear answer, Rust first checks whether sync has already stored Gmail ids on the draft. If so, the create succeeded. Otherwise Rust lists the account's drafts and reads each one it does not know with `drafts.get` (`format=metadata`) before trying again. If one has this draft's id, Rust keeps it and deletes any other copies. Otherwise it creates the draft again. Sync reads the header on every draft it fetches too: a Gmail draft carrying a local draft's id is that draft, not a new one, and Rust stores its Gmail ids on it. If that local draft already has a different live Gmail draft, Rust deletes the extra copy.

## Data

One SQLite file in the app data directory. `rusqlite` with bundled SQLite and FTS5. Rows are keyed by account so two Gmail accounts can share a thread id space without colliding.

| Table | Holds |
|---|---|
| `accounts` | Email, display name, `sendAs` addresses, rail position, avatar hue, `history_id`, last sync time, needs sign-in |
| `threads` | Subject, snippet, sort time, unread, snooze time, `seen_in_inbox`, assigned split, `rev`, Gmail thread id, Gmail `historyId` at last fetch |
| `messages` | Headers, sanitized HTML, plain text, Gmail ids, `Message-ID` and reply ids, list flag, that message's label ids, `rev` |
| `labels` | Account, Gmail label id, name, system or user |
| `thread_labels` | Thread to label, derived from message labels by the rule in Views |
| `attachments` | Filename, mime, size, Gmail attachment id, `Content-ID` for inline parts, local path once downloaded |
| `contacts` | Account, address, latest display name, sent count, received count, seen-only-on-list-mail flag, last contact time |
| `contact_sources` | Account and Gmail message id of every message already counted into `contacts` |
| `drafts` | Compose state, Gmail draft id, Gmail message id and thread id from the last upsert, account, kind (new, reply, forward), target thread and message, started-on-latest flag, kept Gmail quote block, started-fresh flag, `rev` |
| `outbox` | Draft reference, `send_after`, state (waiting, sending, sent), Gmail draft id, Gmail message id once sent, undo requested (a held undo, applied when the send resolves), `rev` |
| `ops` | Per account: queued mutations with the message ids they touch, and the last 50 undo entries. Mail entries hold the labels and `snooze_until` values the action changed, with their values from before it. Send entries reference their outbox row. Pin entries hold the pin they replaced |
| `splits` | Account, name, position, query, hide-when-empty, enabled, and whether it is Important or Other |
| `split_overrides` | Account, and a sender, domain, or label name pinned to a split |
| `messages_fts` | FTS5 over from, to, cc, subject, plain body |
| `meta` | Last action number received and last `rev` |
| `settings` | Key, value, and the account for per-account settings, such as mute |

## UI state

Rust owns mailbox data. The UI keeps a mirror of what is on screen or about to be, in a Zustand store. Rust decides view membership, split routing, sort order, and snooze. The UI's only list logic is the removal table under Actions, and Rust's next patch overrides it.

### Subscriptions

The UI subscribes to what it shows:

- A list: account plus view, or a search query. Rust sends the ids in order and the rows for a window around the viewport. After that it sends patches: ids inserted, removed, or moved, and the full row for every changed id in the list, inside the window or not. Rows outside the window are fetched ahead of scrolling. A Search list holds local matches plus the Gmail hits for that query. A Gmail hit is listed only while the thread has a message outside trash and spam. In Label, Trash, and Spam, threads fetched from Gmail are stored and listed by the view's rule, like any cached thread.
- Inbox is one list. Each id carries its split, so a move between splits is one patch.
- The Drafts list is keyed by draft id, not thread id, so a local draft with no Gmail thread is listed and each draft on a thread has its own row. In Drafts, the highlight and selection hold draft ids, and mail actions do nothing, as `y` does.
- A conversation: keyed by thread and by whether it was opened from Trash or Spam, since that decides which messages are listed. It also says whether it is open or prefetched. Only an open one counts as showing its thread for notifications. Marking read on open is an action the UI sends, never a side effect of subscribing. Opening a conversation sends it after the conversation's first load, or at once if it was prefetched. Which messages are expanded, and where the view scrolls, are fixed from the unread state when the conversation opens. Later read changes, including this one, do not collapse them. A message that arrives while the conversation is open is expanded when it is the new latest message or is unread, and its body loads as for any expanded message. If it arrives unread while the conversation is open in the focused window, the UI marks it read with an action that is not an undo entry. It stays expanded. It holds the messages, drafts, and outbox rows. Messages that open collapsed arrive as their one-line summary. Their bodies load when expanded.
- A composer, keyed by draft id, for as long as it is open. Rust treats that draft as open in a composer for the Drafts rules and sends the `Draft was sent or deleted elsewhere` notice on it. Nothing Rust sends overwrites the editor's content.
- An account summary, one per account, always open: labels, splits, unread counts for the rail and the Dock badge, sync state, outbox rows, the undo history (entry id, kind, thread ids, and for a send its outbox row), notices, and settings: the global ones and that account's own. The Settings sheet and the message iframes read settings from the mirror. Changing remote images re-renders open and prefetched messages. Send toasts read the outbox from here, so they work across conversations and account switches. A notice is a Gmail rejection of a queued action or a send, or a held send undo that resolved as not sent. It carries the error, if any, and for a send its draft id. It persists until the UI shows it as a toast and acks it with an action.

The open conversation is subscribed. On the list, the highlighted conversation and the ones before and after it are subscribed once the highlight has rested for 100 ms, so holding `j` does not load every thread it passes. In an open conversation, the ones before and after it in the list are subscribed once it has been open for 100 ms. Each account keeps its last view subscribed after a switch, so switching back paints from the mirror. Closing a subscription drops its rows unless another subscription holds them. Lookups such as autocomplete are plain Tauri commands. On load, the UI's first call closes every subscription of the previous page, for example after a webview crash. An open composer among them is closed as a second `Esc` would close it.

### Ordering

One Rust thread does every SQLite write: UI actions, sync results, and queue and outbox updates. Each write transaction increments a counter, `rev`, and stamps the rows it touched. `threads`, `messages`, `drafts`, and `outbox` rows each store the `rev` of their last change. A change to a thread's messages, labels, drafts, or outbox rows also stamps its `threads` row. A pin, rule, or split change re-routes the account's Inbox threads in the same transaction and stamps the ones that move.

After each commit, the writer thread sends one message on a Tauri channel shared by all subscriptions: the commit's `rev`, its `ack`, and the patches for every open subscription the commit affects, possibly none. It builds them from the same snapshot, then applies the next job. The mirror applies a whole message at once. A new subscription's first load is a job on the writer thread too. It is sent on the channel as its own message, with the current `rev`, `ack`, and the subscription's ids and rows, so it is ordered with the patches. The subscribe invoke returns nothing. The mirror ignores patches for a subscription until its first load arrives. UI actions and first loads run before any queued sync job. Sync and queue results commit in small transactions of a few milliseconds each, and sanitizing and other parsing happen before a job reaches the writer thread.

Window fetches and bodies are read on separate connections in WAL mode and return each row with its `rev`. Only channel messages carry `ack`. A fetch reply never changes pending actions. The mirror keeps a row, fetched or pushed, unless its own copy has a newer `rev`. A message body never changes, so a loaded body stays with its message whatever the `rev`. Each message row says whether its body is stored. A body read for a message with no stored body returns nothing and is not kept. An expanded message without its body shows a loading line and reads the body when a patch says it is stored. List membership and order come only from the channel, so a stale fetch can never bring back a row that left the list.

### Actions

Each UI action, such as a mail action, undo, label change, pin, or draft save, gets the next action number. Send and discard first flush the composer's pending save into the same invoke, ahead of them. A save that reaches the writer thread after its draft was sent or discarded is dropped. Numbers continue from the last number Rust has received, which the UI asks for on load, so a webview reload does not reuse numbers of actions still queued. Rust stores the last action number and the last `rev` in SQLite. Both continue across restarts and never go back. The UI sends actions one invoke at a time: the next goes when the previous returns, which is as soon as Rust has queued it. Actions taken meanwhile go together in the next invoke, so they reach the writer thread in number order. Subscribing and closing a subscription go through the same queue, so the writer thread sees them in order with the actions.

The UI makes the id of a draft or label it creates, so a fresh composer subscribes and saves at once. Rust keeps that id as the local id. A label create and the label adds made with it go in one action.

A fresh composer's first save carries only its kind, its target thread and message, and for a reply whether it is reply all. In that transaction Rust fills From, To, Cc, Subject, and the quote by the rules in Addressing, and stores them as the draft's defaults for the untouched check. For a forward, Rust also attaches the target message's attachments, and they are part of the defaults. The composer subscribes after its first save, so its first load carries the filled fields. The body is editable at once, and later saves carry the full compose state.

Closing a composer is an action. Anything that takes an open composer off screen, such as a rail click, a notification click, or opening another draft, first closes it as a second `Esc` would. Like send and discard, it carries the composer's pending save ahead of it. Rust applies the untouched rule in that transaction, and the subscription closes with it. If the composer saved no edits, Rust also queues a read of any Gmail change skipped while it was open. The Gmail call runs outside the writer thread, like any sync fetch, and its result is written as a sync job.

A draft keeps its started-fresh flag only while its first composer is open. Closing that composer clears the flag. `Cmd+Q` first closes an open composer as a second `Esc` would, and quits once that action commits. On launch, Rust applies the untouched rule to every draft whose started-fresh flag is still set, for example after a crash.

An action carries its resolved effect, never a toggle: the labels to add and remove on which threads, the snooze time, or the undo entry id. The UI resolves `s`, `u`, and `l` against the mirror. The mirror keeps the rows of selected threads until the selection clears, so it can resolve them even after they scroll out of the window.

In the same frame, the UI applies the direct effect to the mirror: row fields such as read, starred, and labels, and removal from the current list by this table. Every other list change waits for Rust.

| Action | Removes the row from |
|---|---|
| `e` | Inbox, Snoozed |
| `Shift+E` | Done, Snoozed, Trash, Spam |
| `h` | Inbox, Done |
| `#` | Every list except Trash |
| `!` | Every list except Spam |
| Unstar | Starred |
| `y`, or `l` removing the current view's label | The current view |
| Send or discard a draft | Drafts |

An undo entry's id is the number of the action that made it. `z` takes the newest entry in the account summary with pending actions applied: a pending undoable action adds its entry, and a pending undo removes the entry it targets. So a quick `e` then `z` undoes that `e`, and a second quick `z` undoes the entry before. A toast's Undo sends its own entry's id. Rust undoes that entry, or rejects the action if the entry is gone, so two quick undos never undo two entries by mistake. For an entry made this session, the mirror keeps each removed row and the id of the row before it, and puts the row back there. Otherwise the row waits for Rust.

Every channel message carries `ack`, the highest action number applied in its snapshot. The mirror holds the state Rust confirmed plus the actions after `ack`, and shows those actions applied on top. A message from Rust updates the confirmed state, drops the actions up to its `ack`, and re-applies the rest. If Rust rejects an action, `ack` still moves past it, so the mirror falls back to Rust's state. Pending actions last only until the writer thread commits them, usually a few milliseconds. Actions still waiting on Gmail live in the SQLite queue, and the data Rust sends already includes them. Rows the user just acted on never flash back during fast triage or a poll.

A second Zustand store holds only chrome: account on screen, each account's last view and highlight, selected ids, palette open, search query, compose mode.

The thread list is virtualized. The conversation view does not virtualize a thread of normal length.

## Security

- Refresh tokens live in Keychain. The database holds no tokens.
- HTML is sanitized before insert. The message iframe's CSP is `script-src 'none'`.
- The app CSP allows inline styles and `img-src https: http: data:`, because `srcdoc` iframes inherit it. Tauri's nonce injection is turned off for `style-src` (`dangerousDisableAssetCspModification`), because a nonce makes browsers ignore `'unsafe-inline'`.
- The webview has no Gmail credentials and cannot call Gmail.
- OAuth uses the system browser via loopback, then returns to the app.

## Stack

| Layer | Choice |
|---|---|
| Shell | Tauri 2.12. Rust stable, 1.90 or newer |
| UI | React 19.3, TypeScript 7.0.2 |
| Style | Tailwind CSS 4, shadcn/ui on Base UI, Lucide, Sonner toasts |
| Palette | cmdk 1.1 |
| List | TanStack Virtual |
| Mailbox state | Subscriptions pushed by Rust, mirrored in a Zustand 5 store. See UI state |
| Chrome state | Zustand 5 |
| Editor | TipTap 3 |
| Database | rusqlite 0.40, bundled SQLite, FTS5 |
| Tokens | keyring, macOS Keychain |
| HTML | ammonia |
| MIME | mail-builder, for every message and draft Rust writes |
| IPC types | ts-rs 12, generated from Rust |
| Gmail | reqwest and the oauth2 crate |
| Plugins | notification, opener, window-state, single-instance |

Tauri commands are a small handwritten TypeScript surface. Every type that crosses it is generated from the Rust types with ts-rs into `src/bindings/`: actions, channel messages, rows, notices, and command arguments and results. No IPC type is written by hand in TypeScript. `cargo test` regenerates the bindings, and the check fails if they differ from the committed files. The UI does not open SQLite itself.

Tooling is Vite+ 1.4. `vite.config.ts` configures Oxfmt, Oxlint with `typeAware` and `typeCheck`, and Vitest. `vp check` is the frontend gate. `typescript@7.0.2` is installed for the editor. The `vite` package resolves to `@voidzero-dev/vite-plus-core@1.4.0`, and `vitest` is pinned to the copy Vite+ ships. pnpm is the package manager under `vp`. Rust uses rustfmt and clippy.

Tauri's dev and build commands are `vp dev` and `vp build`.

## Testing

Gates before a commit: `vp check`, `cargo test`, `cargo clippy`, and the end-to-end suite, all run by `vp run gate`. Before a release, the performance budgets on the perf fixture.

- End to end: Playwright drives the built UI in WebKit. The UI talks to the real Rust core through the web bridge, and the core talks to the fake Gmail. A test server controls app time, waits for the core to go idle, restarts or crashes the app, and records what the native shell would do: notifications, the Dock badge, opened URLs and files, and stored tokens. `e2e/COVERAGE.md` maps each user case in this spec to a test. Tauri's WebDriver has no macOS support, so this is the end-to-end path.
- Unit tests only for complex logic the end-to-end suite can't pin down: the sanitizer against hostile HTML, MIME parsing that never panics, the split query language and snooze phrases, the Gmail scheduler, and the OAuth state check, shared refresh, and secrets kept out of debug output.
- Writer and channel: Rust property tests with proptest. They generate random sequences of UI actions, sync results, subscribes, and closes. After every channel message, a list built from its first load plus patches equals a fresh first load at that `rev`, and a model mirror never shows a row the user just removed coming back while its action is pending.
- Fake Gmail: a local HTTP server (`ub-fake-gmail`) that serves the endpoints the app uses, with stored state for labels, threads, messages, drafts, history, and `sendAs`. It can inject each outcome in Gmail errors: no connection, no response after the request is sent, 401, 429, 5xx, 404, and other 4xx. Debug builds read the Gmail and Google OAuth base URLs from config, so token refresh is tested too. The end-to-end suite runs against it.
- Real Gmail: the Done when list is run by hand against two test Google accounts before v1.

## Build order

The acceptance bar is the whole daily driver. Build it in this order so each step is usable. Each step meets the performance budgets for what it adds.

1. Theme tokens, window, list and conversation view, fake threads and the perf fixture, shortcuts, palette, shortcut sheet, the keydown-to-paint meter, the writer thread and channel with their tests.
2. The fake Gmail server. One real Gmail account: OAuth, sync, Gmail error handling, read with collapsed messages, archive.
3. Reply composer, reply all, forward, compose, recipient autocomplete, attachments, undo send.
4. More accounts and switching, labels, search.
5. Split Inbox: Important, Other, custom splits, pins, `Tab`.
6. Snooze, offline queue, notifications, Dock badge.

## Done when

- Two Gmail accounts can be added, switched with `Ctrl+1` and `Ctrl+2`, and removed. Tokens are in Keychain.
- Each account's Inbox matches Gmail's inbox after a sync, including mail that arrived while the app was closed.
- Inbox rows sit under Important, Other, and any custom splits. `Tab` moves between them. A sender pinned to Important stays there across syncs. An empty split with hide-when-empty on stays hidden, except when `g` then `i` or `g` then `o` shows its header.
- `j`, `k`, `e`, `h`, `c`, `r`, `Enter`, `/`, and `Cmd+K` do what the shortcut table says.
- Every budget in Performance holds on the perf fixture.
- Archive, label, star, trash, and snooze update the list before the network returns, and they show up in Gmail.
- A long thread opens with older read messages collapsed, scrolled to the first unread message.
- Reply opens under the thread with the thread still visible. Typing a name in To suggests people this account has written to.
- A send can be undone from the toast. A failed send retries from the outbox.
- Airplane mode: cached threads open, search hits cached mail, and an archive or a reply lands after the network returns.
- A new inbox message in either account raises a notification and increments the Dock badge.
- A hostile HTML message cannot run script in the app window.
