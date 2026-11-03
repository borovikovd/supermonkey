# E2E coverage design

This is the test design for the end-to-end suite. It is not test code. It says which cases the suite needs, how each one runs, and what it must observe. Several engineers can write the specs in parallel from it, and a reviewer can use it to check that the suite is sufficient.

- **What runs.** Playwright on WebKit drives the real React UI. The UI talks to the real Rust core through the web bridge. The core talks to the in-process fake Gmail (`ub-fake-gmail`), which can inject failures.
- **What is not exercised.** The native shell (window chrome, real macOS notifications, the real Dock, the real Keychain). A recording platform stands in for it, so notifications, the Dock badge, opened URLs and files, and stored tokens can still be asserted.
- **Source of truth.** `SPEC.md`, as amended by `DEVIATIONS.md`. Section references below (§) are SPEC headings.
- **Where the spec is silent or ambiguous,** a case does not guess. The question is listed under [Decisions on open questions](#10-decisions-on-open-questions), and the case says which assertion waits on it.

Contents:

1. [Conventions](#1-conventions)
2. [Seed scenarios](#2-seed-scenarios)
3. [Harness requirements](#3-harness-requirements)
4. [Test files and case counts](#4-test-files-and-case-counts)
5. [Cases](#5-cases)
6. [Done-when traceability](#6-done-when-traceability)
7. [SPEC section traceability](#7-spec-section-traceability)
8. [Not covered end-to-end, and why](#8-not-covered-end-to-end-and-why)
9. [Kept unit and property tests](#9-kept-unit-and-property-tests)
10. [Decisions on open questions](#10-decisions-on-open-questions)
11. [Selector notes and testability gaps](#11-selector-notes-and-testability-gaps)

---

## 1. Conventions

**Case ids.** Each id is a file prefix and a number, for example `TRI-03`. Cases go in the file named in [section 4](#4-test-files-and-case-counts). Keep the id in the Playwright test title, for example `test("TRI-03 trash …")`, so the reviewer can trace each case.

**Priority.**

| Priority | Meaning                                                                                                                                                                                                                                                     |
| -------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **P1**   | Must have. Covers a SPEC "Done when" item, a risk of losing or duplicating data (a lost draft, a double send, delivered mail trashed, a queued action dropped), a security property, or a core state machine. The suite is not sufficient without every P1. |
| **P2**   | An explicit SPEC rule that has no data-loss risk, or a second partition of a P1 behavior.                                                                                                                                                                   |
| **P3**   | A boundary or edge case that costs a lot to set up (big seeds, real quota), or a detail that is close to cosmetic.                                                                                                                                          |

**Technique.** Each case names how it was derived:

- **EP**: equivalence partitioning
- **BVA**: boundary values
- **ST**: state transitions (one test walks the machine)
- **DT**: a decision table (one test, or one parametrized test, per table)
- **EG**: error guessing

**Format.** Each case has a header line, `#### ID · Priority · title`, followed by these fields:

- _Technique and source_
- **Seed:** the scenario from section 2, plus any extra setup
- **Steps:** keys, clicks, and control-API calls
- **Expect:** what the UI shows, and where it matters, what Gmail holds

**How to assert.**

- **UI first, then Gmail.** Check what the user sees. Then call `settle()` and read Gmail state with `gmail.gmail(email)`.
- **"Before the network returns."** Some SPEC rules say the UI must update before Gmail answers (Done when, item 6). Test these by parking the Gmail request with a `hold` fault, asserting the UI, then releasing the request. Never use sleeps to test this.
- **Time** moves only through `app.advance(ms)`, which moves the core and the browser together. Tests never wait on the wall clock.
- **Negative assertions** ("nothing happens", "no request", "no toast") need `settle()` first. Without it they pass vacuously.
- **"Error toast"** means a Sonner toast (`[data-sonner-toast]`) in the error style. **"No toast"** means no new toast appeared after the step.
- **Gmail-side "label on a thread"** follows §Views: some message that is not in trash or spam carries the label.

**Shorthand used in steps.**

| Shorthand                         | Meaning                                                                                                                      |
| --------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| `A`, `B`                          | The accounts `ana@acme.test` and `ben@bolt.test` (see section 2)                                                             |
| `T.key`                           | A seeded thread, by its key in section 2                                                                                     |
| `open(T)`                         | Highlight T's row and press `Enter`                                                                                          |
| `hold(m)` / `release(m)`          | Park every request to Gmail method `m` until released (`gmail.hold(m)`, then `hold.release()`)                               |
| `settle()`                        | Wait until the core is idle and the UI has applied the latest channel message (`app.settle()`)                               |
| `poll()`                          | `gmail.poll()`: a history poll runs now for every account                                                                    |
| `fault(m, outcome, {nth, times})` | `gmail.fault(...)` on Gmail method `m`                                                                                       |
| `offline(on)`                     | `gmail.offline(on)`                                                                                                          |
| `other(...)`                      | A change made in Gmail by another client: `gmail.modify`, `gmail.deleteMessage`, `gmail.label`, the draft helpers, and so on |
| `req(m)`                          | The request log, filtered to Gmail method `m`                                                                                |

---

## 2. Seed scenarios

A scenario is a function in `e2e/seeds.ts`. It builds fake-Gmail state through the control API: `gmail.account`, `gmail.user`, `gmail.deliver({poll: false, date})`, `gmail.label`, `gmail.sendAs`, and the Gmail-draft helpers. It then calls `app.open()`, which runs `start` and waits for the connect sync to settle. A scenario takes `{ connect?: string[] }`, which says which seeded identities are already signed in. The default is all of them.

Every scenario runs with:

- **Clock:** Monday 2026-11-02 10:00 in America/New_York, the same anchor the kept snooze unit tests use. DST ended on 1 November, so the test week has no DST change.
- **Time zone:** pinned on both sides, the core (`TZ`) and the browser (`timezoneId`).
- **Mail dates:** messages are dated relative to the clock.

### Identities

| Key | Address         | Primary `sendAs` display name                                                                        | Other `sendAs`                    | User labels                                                             |
| --- | --------------- | ---------------------------------------------------------------------------------------------------- | --------------------------------- | ----------------------------------------------------------------------- |
| `A` | `ana@acme.test` | Ana Ames                                                                                             | `ana@ames.dev` (alias, same name) | Receipts, Travel, Projects, Work/Clients                                |
| `B` | `ben@bolt.test` | **Zed Bolt** (differs from the address, so a test can see that the display name comes from `sendAs`) | –                                 | Family                                                                  |
| `C` | `cat@cove.test` | Cat Cove                                                                                             | –                                 | – (a Gmail user that exists but is not signed in; used for Add account) |

### `none`

`A`, `B`, and `C` exist in fake Gmail with `standard` mail, but the app has no accounts. The app opens on the first-run screen.

### `empty`

`A` is signed in. It has no mail and only system labels. `B` is not seeded.

### `standard`

`A` and `B` are signed in. `A` has these threads. Unless noted, each message is `INBOX` with `CATEGORY_PERSONAL` and was received in the last 7 days.

| Key             | Content                                                                                                                                                                                                                                                                                                                    | Read state                 | Expected split                       |
| --------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------- | ------------------------------------ |
| `plan`          | "Q4 plan", 12 messages. Carol `carol@corp.test` and Dave `dave@corp.test` alternate, with two own replies from A (`SENT`). m12 is from Dave.                                                                                                                                                                               | m1–m9 read, m10–m12 unread | Important                            |
| `fold`          | "Fold check", 11 messages from Eve `eve@corp.test`                                                                                                                                                                                                                                                                         | only m5 unread             | Important                            |
| `addr`          | "Offsite venue". From Carol. To `ana@acme.test`, `dave@corp.test`. Cc `erin@corp.test`. No Reply-To.                                                                                                                                                                                                                       | unread                     | Important                            |
| `replyto`       | "Ticket #42". From Ivy `ivy@help.test`, `Reply-To: tickets@help.test`. To `ana@acme.test`. Cc `jo@help.test`.                                                                                                                                                                                                              | read                       | Important                            |
| `alias`         | "Guild meetup". From Gus `gus@guild.test`. To `ana@ames.dev`, `hal@guild.test`.                                                                                                                                                                                                                                            | read                       | Important                            |
| `own`           | "Budget draft". m1 from Kim `kim@kite.test` (INBOX). m2, the latest, is From `ana@acme.test` To `kim@kite.test` Cc `lee@kite.test` (`SENT`).                                                                                                                                                                               | read                       | Important (rules read Kim's message) |
| `ownAlias`      | "Alias note". m1 from Kim (INBOX). m2, the latest, is From `ana@ames.dev` To Kim (`SENT`).                                                                                                                                                                                                                                 | read                       | Important                            |
| `fwd`           | "Contract". From Alice `alice@law.test`. HTML with Gmail-style quoted history (`gmail_quote`), an inline `cid:` image `logo.png`, and an attachment `contract.pdf` (40 KB, fixed bytes).                                                                                                                                   | read                       | Important                            |
| `links`         | "Links". From Lou `lou@site.test`. HTML links: `https://example.org/a`, `mailto:x@example.org`, `https://example.org/b`, one with `target="_top"`. Remote `<img src="http://<fake>/img/pixel.png">`, CSS `background-image:url(http://<fake>/img/bg.png)`, and one `data:` image.                                          | read                       | Important                            |
| `trashedLatest` | "Latest rule". m1 from Mia `mia@m.test` (INBOX, read). m2, newer, from Ned `ned@n.test`, labeled `TRASH`.                                                                                                                                                                                                                  | read                       | Important                            |
| `p1`…`p6`       | "Person 1"…"Person 6", one message each, from Pia, Quin, Rae, Sol, Tom, and Uma (`@people.test`). The body of `p3` contains "quokka".                                                                                                                                                                                      | p1, p3, p5 unread          | Important                            |
| `news1`…`news3` | Digests from `news@letters.test` and `digest@letters.test`, with `List-Unsubscribe` (news1, news3) or `List-Id` (news2). news1 and news3 carry `CATEGORY_UPDATES`. news2 carries `CATEGORY_PERSONAL`, so list mail beats the category. All three are addressed To `subscribers@letters.test`, not to any of A's addresses. | unread                     | Other                                |
| `promo`         | From `deals@shop.test`. `CATEGORY_PROMOTIONS`, no list headers.                                                                                                                                                                                                                                                            | read                       | Other                                |
| `social`        | From `notify@social.test`. `CATEGORY_SOCIAL`.                                                                                                                                                                                                                                                                              | read                       | Other                                |
| `forum`         | From `group@forum.test`. `CATEGORY_FORUMS`.                                                                                                                                                                                                                                                                                | read                       | Other                                |
| `receipt`       | From `billing@shop.test`. `CATEGORY_PROMOTIONS` and the user label Receipts.                                                                                                                                                                                                                                               | read                       | Other                                |

`A` also has threads outside Inbox. The connect sync is expected to fetch only some of them.

| Key                          | Content                                                                             |
| ---------------------------- | ----------------------------------------------------------------------------------- |
| `starredOld`                 | `STARRED` and Travel, not in Inbox, 30 days old                                     |
| `sent10`, `sent89`, `sent91` | Mail A sent to Kim 10, 89, and 91 days ago (boundary of the 90-day connect window)  |
| `archivedOld`                | "Archived before", read, not in Inbox, 20 days old (never seen in Inbox by the app) |
| `projOld1`…`projOld3`        | Label Projects, not in Inbox, 200 days old                                          |
| `trashOld`                   | `TRASH`, 40 days old                                                                |
| `spamOld`                    | `SPAM`, 40 days old                                                                 |
| `invoiceOld`                 | "Invoice 2019", archived, 400 days old (reachable only by search)                   |

`B` has `b1`…`b4` (b1 and b2 unread; b4 labeled Family). It also has `collide`, "B collide", whose **Gmail thread id is the same as A's `p1`** (rows are keyed by account, §Data).

`C` exists only in Gmail, through `gmail.user`. It has 3 unread Inbox threads, `c1`…`c3`.

`standard` has no drafts.

### `drafts`

`standard`, plus Gmail-made drafts on `A`, created in fake Gmail before connect:

| Key              | Draft                                                                                                                                        |
| ---------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| `dNew`           | "Idea", alone in its own thread, no `In-Reply-To`                                                                                            |
| `dFwd`           | "Fwd: Contract", in the `fwd` thread, `In-Reply-To` Alice's message                                                                          |
| `dLatest`        | A reply in `p4`. `In-Reply-To` is p4's latest message. HTML is "My text" followed by a `gmail_quote` block. It has an attachment `spec.pdf`. |
| `dEarly`         | A reply in `plan`, `In-Reply-To` plan's m3                                                                                                   |
| `dUnknown`       | A reply in `p6`, `In-Reply-To <nobody@x.test>` (matches no message)                                                                          |
| `dTwoA`, `dTwoB` | Two reply drafts in `p2`, both aimed at p2's latest message. dTwoB is newer.                                                                 |

### `contacts`

`A` is signed in, with mail built to exercise autocomplete. Every name starts with "S" so that one query covers all the tiers.

| Address                                       | How it appears in the mail                                                                    | Expected                                |
| --------------------------------------------- | --------------------------------------------------------------------------------------------- | --------------------------------------- |
| `sam@x.test` (Sam Stone)                      | A sent 5 messages to it in the last 30 days                                                   | suggested, sent tier                    |
| `sara@x.test` (Sara Swift)                    | A sent 1 message to it, 200 days ago (outside the 90-day sync, so found only by the backfill) | suggested after the backfill, sent tier |
| `sue@recv.test` (Sue Sender)                  | From address on 2 received messages                                                           | received tier                           |
| `sid@cc.test` (Sid Copy)                      | Cc on a received, non-list message                                                            | co-recipient tier                       |
| `sly@list.test` (Sly List)                    | To/Cc only on list mail; A never sent to it                                                   | never suggested                         |
| `noreply@svc.test`, `no-reply@svc.test` (Svc) | From address on received mail                                                                 | never suggested                         |
| `spammy@spam.test` (Spam Sam)                 | Appears only in a `SPAM` message                                                              | never suggested                         |
| `drafty@d.test` (Sy Draft)                    | Appears only in a Gmail draft                                                                 | never suggested                         |
| `ana@ames.dev`                                | A's own alias, on received mail                                                               | never suggested                         |

`contacts` also has eight "Mark Mxxx" contacts that A sent to, for the six-suggestion cap.

### `splits`

`standard`, plus two more threads on `A`:

- `X`: two messages from `x@corp.test`, person-to-person. m1 carries the user labels Proj and Ops. m2, the latest, carries no user label.
- `Y`: one message from `y@corp.test`.

### `security`

`A` is signed in, with these threads:

- `hostile`: a message whose HTML contains `<script>`, `<img onerror>`, `<svg onload>`, `<body onload>`, `href="javascript:…"`, a `<form action="http://evil.test">` with a submit button, `<iframe src="http://evil.test">`, `<meta http-equiv=refresh content="0;url=http://evil.test">`, `<base href>`, and a CSS `url(javascript:…)`. Every payload sets `top.__pwned` or `parent.__pwned`.
- `hostileMeta`: subject `<img src=x onerror=top.__pwned=1>`, From display name `<b>Boss</b>`, and a snippet that contains `<script>`.

### `bulk`

`A` is signed in, with:

- 60 Inbox threads: 40 Important and 20 Other.
- 40 threads labeled Archive2019, not in Inbox.
- 40 archived threads older than 90 days with "invoice" in the subject. These exist only in Gmail until a search finds them.

`bulk` needs the quota override (H12).

### `bulk1000`

`A` has 999 Important Inbox threads. It needs the quota override. Used only by the P3 count-cap case.

---

## 3. Harness requirements

A harness is already committed: `e2e/control.ts`, `e2e/app.ts`, `e2e/fixtures.ts`, and `src-tauri/src/engine/testserver.rs`. It works as follows:

- Each test gets its own `e2e-server`, with a fresh database and a fresh fake Gmail, and the server also serves the built UI.
- The `gmail: Control` fixture is the control API.
- The `app: App` page object drives the UI.

All of them exist now; [section 3.1](#31-using-the-harness) says how to use them. The table marked each requirement as **exists** or **needed**. The cases depend on every **needed** item, and the case ids that need it are listed.

| #   | Requirement                                                                                                                                                                                                                                                                                                                                                                                                                 | Status                          | Needed by                                                                               |
| --- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------- | --------------------------------------------------------------------------------------- |
| H1  | **Per-test server and seeding:** `account`, `user`, `deliver({poll, date, threadId, inReplyTo, listId, headers, labels, attachments})`, `label`, `sendAs`, then `start` and `app.open()`                                                                                                                                                                                                                                    | exists                          | all                                                                                     |
| H2  | **Named seed scenarios** in `e2e/seeds.ts` (section 2), built on H1. A `dates` helper relative to the clock.                                                                                                                                                                                                                                                                                                                | exists (`seeds.ts`)             | all                                                                                     |
| H3  | **Read Gmail state:** messages with label ids, headers, and raw MIME; drafts with message headers (to read `X-Ubermensch-Draft`); labels; `historyId`                                                                                                                                                                                                                                                                       | exists (`gmail.gmail(email)`)   | most                                                                                    |
| H4  | **`sendAs` in Gmail state**, and a parsed view of the raw MIME tree (content types, Content-ID, attachment bytes)                                                                                                                                                                                                                                                                                                           | exists (`mime.ts`)              | REP-03, REP-06, ATT-01, ATT-04                                                          |
| H5  | **Changes by another Gmail client:** `modify`, `deleteMessage`, `label`, `sendAs`                                                                                                                                                                                                                                                                                                                                           | exists                          | NAV-07, SYN-03, SNZ-05, …                                                               |
| H6  | **Gmail drafts from another client:** `createDraft(email, {threadId, inReplyTo, subject, html, attachments})`, `updateDraft`, `deleteDraft`, `sendDraft` (the "sent from a phone" case). Plus `deleteThread`.                                                                                                                                                                                                               | exists                          | `drafts` seed, REP-09, DRF-03, DRF-04, ERR-06, SYN-04                                   |
| H7  | **Faults:** `fault({method, path, nth, times}, {drop, noResponse, status(+reason, retryAfterSecs), delay})`, `clearFaults`, global `offline(on)`                                                                                                                                                                                                                                                                            | exists                          | ERR-_, SND-_, OFF-*                                                                     |
| H8  | **Fault additions:**                                                                                                                                                                                                                                                                                                                                                                                                        | exists                          | TRI-01, LAB-01, SNZ-03, SND-05, SND-08, ERR-03, ERR-07                                  |
|     | (a) `hold`, which parks matching requests until `release(match)`. It keeps "before the network returns" and "row is sending" deterministic.                                                                                                                                                                                                                                                                                 |                                 |                                                                                         |
|     | (b) `statusAfterApply(5xx)`: Gmail does the write, then answers 5xx. Needed for `drafts.create` and `drafts.send`.                                                                                                                                                                                                                                                                                                          |                                 |                                                                                         |
|     | (c) A matcher by account and by query parameter, for example `format=full` or `q=` (the path glob alone cannot tell accounts apart when the path says `users/me`).                                                                                                                                                                                                                                                          |                                 |                                                                                         |
|     | (d) Infinite `times`.                                                                                                                                                                                                                                                                                                                                                                                                       |                                 |                                                                                         |
| H9  | **Request log:** a list of entries, each with method name, account, path, query, JSON body, order, virtual time, and outcome. `requestCount` alone cannot assert order, ids, `q`, `maxResults`, or `includeSpamTrash`.                                                                                                                                                                                                      | exists (`gmail.requests`)       | TRI-11, OFF-02, SRC-02, SRC-04, VIEW-05, SRC-07, SND-01, ATT-02, ERR-02, ERR-06, ERR-07 |
| H10 | **Expire tokens and history:** `expireAccessTokens(email)`, and `expireHistory(email)` to raise the history floor so `history.list` returns 404                                                                                                                                                                                                                                                                             | exists                          | ERR-05, ACC-08, SYN-04, SPL-04                                                          |
| H11 | **OAuth:** `nextSignIn({email, abandon})` and `invalidGrant(email, on)` exist. **Needed:** `nextSignIn({deny: true})`, which returns Google's `access_denied`; `keychain()`, which lists stored token items `{service, account}`; and `revocations()`, which lists refresh tokens revoked at Google. The auth URL is already recorded in `platform().openedUrls` (scope, PKCE, and `redirect_uri` can be asserted from it). | exists                          | ACC-01, ACC-07, ACC-08, ACC-09                                                          |
| H12 | **Quota override** in the E2E server, a runtime knob, so that `bulk`, `bulk1000`, and large syncs finish in seconds. One case runs at the production budget.                                                                                                                                                                                                                                                                | exists (`gmail.quota`)          | SRC-07, SPL-09, SPL-10, UIC-01, NAV-02                                                  |
| H13 | **Clock:**                                                                                                                                                                                                                                                                                                                                                                                                                  | exists                          | SNZ-_, SND-_, ERR-02/03, SYN-03, SRC-04                                                 |
|     | (a) **Absolute start time.** Set the core and browser clocks to the scenario anchor before `start`. Today the clock can only advance from real now.                                                                                                                                                                                                                                                                         | exists                          |                                                                                         |
|     | (b) **Everything time-based runs on app time:** the 45 s poll timer, retry back-off, `Retry-After`, the outbox `send_after`, the 30 s sync-status threshold, and the search debounce. Today `advance` kicks snoozes and queues only; poll and back-off use real `tokio` time.                                                                                                                                               | exists                          |                                                                                         |
|     | (c) `app.advance(ms)`                                                                                                                                                                                                                                                                                                                                                                                                       | exists                          |                                                                                         |
| H14 | **Sleep and wake:** `gmail.wake()`, or a clock _jump_ that does not fire the timers in between. This simulates the Mac sleeping. DEVIATIONS says wake is detected as a gap of more than three poll intervals.                                                                                                                                                                                                               | exists (`app.sleep`)            | SNZ-04                                                                                  |
| H15 | **Restart and crash:**                                                                                                                                                                                                                                                                                                                                                                                                      | exists                          | OFF-03, UND-06, SND-09, SNZ-04, DRF-08, SYN-02, SET-02                                  |
|     | `restart()`: a graceful quit as `Cmd+Q` (the UI closes an open composer as a second `Esc` would, then the core stops), followed by a relaunch on the same database and the same fake Gmail.                                                                                                                                                                                                                                 |                                 |                                                                                         |
|     | `crash()`: the core is killed with no composer close.                                                                                                                                                                                                                                                                                                                                                                       |                                 |                                                                                         |
|     | Fake-Gmail state and the clock must be changeable while the app is down. The web build needs a hook in place of the Tauri `quit-requested` event.                                                                                                                                                                                                                                                                           |                                 |                                                                                         |
| H16 | **Webview reload:** `page.reload()` against a live core. The new page's `reset` must close the old page's subscriptions.                                                                                                                                                                                                                                                                                                    | exists (browser)                | DRF-08, UIC-02                                                                          |
| H17 | **Recording platform:** `platform()` returns notifications `{title, body}`, `badge`, `openedUrls`, `openedFiles`, and `focusRequests`.                                                                                                                                                                                                                                                                                      | exists                          | NOT-*, ATT-02, SEC-03                                                                   |
|     | **Needed:** each notification also records its account and thread, and `clearRecorded()` resets the record.                                                                                                                                                                                                                                                                                                                 |                                 |                                                                                         |
| H18 | **Window focus:** `gmail.focus(bool)` exists and returns the `openThread` of the notification-click heuristic. **Needed:** the browser page's `document.hasFocus()` must match, either through Playwright focus emulation or a page hook. The web UI must also act on `openThread` the way the Tauri `notification-open` listener does.                                                                                     | exists (`app.focus`)            | CONV-09, NOT-02, NOT-06                                                                 |
| H19 | **Links from message iframes:** in web mode, `MessageFrame` calls `window.open`. Tests catch it as a popup event, or the web build routes it to an `open_url` command so `openedUrls` records it.                                                                                                                                                                                                                           | exists (`open_url`)             | CONV-12, SEC-03                                                                         |
| H20 | **Remote image host** on the fake (`/img/*.png`, `/img/bg.png`) and a sink for `evil.test`, both with a hit log                                                                                                                                                                                                                                                                                                             | exists                          | SET-02, SEC-01                                                                          |
| H21 | **Introspection,** read-only: `inspect.rows(email)` (row counts per table for one account), `inspect.cacheFiles(email)` (downloaded attachments), and `inspect.dbContains(text)` (scans the SQLite file for a token string). Use it only where the SPEC promises something that has no UI.                                                                                                                                  | exists (`dbContains` only, Q21) | ACC-07, SEC-04                                                                          |
| H22 | **Idle detection:** `settle()`. Wait until no Gmail request is in flight (held ones excepted), the op queues and outboxes are empty or blocked (offline, held, backing off, or waiting on sign-in), the writer is idle, and the mirror has applied the latest `rev`. Without it, negative assertions are flaky.                                                                                                             | exists (`app.settle`)           | most                                                                                    |
| H23 | **UI helpers:** a clipboard-paste helper that dispatches `ClipboardEvent` with text, file fixtures for `filechooser`, and a "rows never reappear" `MutationObserver` probe                                                                                                                                                                                                                                                  | exists                          | RCP-04, ATT-01, UIC-01                                                                  |
| H24 | **Deterministic UI handles,** where the DOM has no semantic marker today (see section 11): `data-selected` on rows selected with `x`, `data-unread` on unread rows, `aria-current` on the selected message, `aria-invalid` on invalid chips, a `data-chip` on chips (`app.chips` already expects it), a split-header role, and a rendered sync status                                                                       | exists                          | TRI-06, NAV-01, CONV-05, RCP-05, ERR-03                                                 |

### 3.1 Using the harness

The API is documented where it lives: `e2e/control.ts` (the `gmail` fixture), `e2e/app.ts` (the `app` fixture), `e2e/seeds.ts` (scenarios, identities `A`/`B`/`C`, `dates`, `ANCHOR`), `e2e/mime.ts` (`parseMime`, `shape`, `htmlOf`, `attachments`), and `e2e/files.ts` (fixed file contents). `e2e/harness.spec.ts` shows each piece in use. What a case writer needs to know beyond the doc comments:

- **Seeds** return a `Seeded`: `s.thread("p1")` (Gmail thread id, subject, messages), `s.message("plan", 3)`, `s.latest("p4")`, `s.label(A, "Travel")`, `s.draft("dLatest")`. Lookups throw on unknown keys. Seeds open the app and settle; pass `{ open: false }` to add state first and call `app.open()` yourself.
- **Time** is frozen until moved. `app.advance(ms)` settles first (so what the user did happens before time moves), then fires every timer due by the new time once, at that time, in the core and the browser. Use `{ stepwise: true }` to fire timers one by one at their own times, so a back-off runs every retry it would in the span. `app.advanceTo(dates.at("2026-11-02T18:00"))` moves to a wall time. `app.sleep(ms)` moves the wall clock only (the Mac asleep); `app.sleepAndWake(ms)` then lets the next poll notice the gap. The browser's `Date` always equals app time; UI timers (toast dismissal, autosave, prefetch) still run in real time.
- **The Gmail search waits for 300 ms of app time**, so a search needs `app.advance(300)` before Gmail results arrive.
- **`settle()`** waits for the core (requests held by a hold excepted), the writer, and the page (invokes, buffered actions, a scheduled autosave or prefetch, the latest channel message). Call it before negative assertions and before reading Gmail.
- **Holds:** `const h = await gmail.hold("messages.batchModify")`, act, assert, then `await h.release()` or `h.release({ kind: "status", status: 400 })`. `release` first waits until the hold has parked a request, so it cannot overtake it. `gmail.holdAnswer(api)` does the work at once and parks only the answer, so the app gets a stale reply (OFF-07).
- **Parking the page's own invokes:** `const release = await app.park("add_attachment")` holds the page's `/invoke/<cmd>` requests (optionally only those whose JSON body passes a filter) until `await release()`. Use it for races between the UI and its own core calls; `settle()` waits on a parked invoke, so release first. To hold UI timers (the autosave) still meanwhile, `page.clock.pauseAt(await app.now())`; `settle()` needs animation frames, so call `page.clock.resume()` and `page.clock.setFixedTime(await app.now())` before it.
- **Faults** take a method name or a `Match` (account, query parameters, `nth`, `times`; unlimited by default). Outcomes: `drop`, `noResponse`, `status`, `statusAfterApply`, `delay`, `omitThreads`. `clearFaults()` and `offline(false)` retry what is backing off at once, as a network change would.
- **Changes by another client** (`modify`, `deleteMessage`, `deleteThread`, `label`, `sendAs`, `createDraft`, `updateDraft`, `deleteDraft`, `sendDraft`) do not poll; call `gmail.poll()` (or pass `{ poll: true }`). `deliver` polls unless `poll: false`.
- **The request log:** `gmail.requests("messages.batchModify", { account: A })` gives, in order, the method name, account, `params` (repeated keys joined by commas), JSON `body`, app time `at`, `held`, `answer` (status, `dropped`, or `noResponse`), `origin`, and `userAgent`.
- **Composers:** `app.compose()` and `app.reply("r" | "Enter" | "f")` wait until the composer is ready. Keys typed before the editor has focus go to the shortcut handler, and fields typed before the draft's first load are overwritten.
- **Restart and crash:** `app.restart(async () => { /* while down */ })` quits as `Cmd+Q` does (an open composer closes first) and relaunches on the same database, fake Gmail, and clock; `app.crashAndRelaunch(...)` skips the composer close. Fake Gmail, holds, and `app.advance` all work while the app is down.
- **Focus:** `app.focus(false)` sets the page's `document.hasFocus()` and the core's focus rules together; `app.focus(true)` returns the notification the focus counts as a click on, and the page opens it as the app does.
- **Platform:** `gmail.platform()` records notifications (`title`, `body`, `account` email, Gmail `thread`), the badge, opened URLs (message links go through the core's `open_url`) and files; `gmail.clearRecorded()` resets it. `app.popups` lists popup windows.
- **The web:** the remote image host is `${gmail.url}/img/<name>.png`; requests to `http(s)://evil.test/...` are routed to the server's sink. Both appear in `gmail.hits()`.
- **Quota:** the server runs at a budget that never waits; `gmail.quota(true)` before `app.open()` selects the production budget.

---

## 4. Test files and case counts

Files are independent and can be written in parallel. Each depends only on the seeds (section 2) and the harness items listed.

| File                       | Prefix | Scope                                                                                        | Main seeds                               | Harness beyond H1–H3        | Cases   | P1      | P2     | P3    |
| -------------------------- | ------ | -------------------------------------------------------------------------------------------- | ---------------------------------------- | --------------------------- | ------- | ------- | ------ | ----- |
| `accounts.spec.ts`         | ACC    | first run, add, switch, avatars, remove, re-auth, isolation                                  | `none`, `standard`                       | H10, H11, H15, H21          | 13      | 9       | 4      | 0     |
| `inbox-navigation.spec.ts` | NAV    | rows, `j`/`k`, open and close, `g` jumps, rail, empty states                                 | `standard`, `bulk`, `empty`              | H12                         | 7       | 4       | 3      | 0     |
| `views.spec.ts`            | VIEW   | latest-message rule, view membership, Done, Drafts, Label/Trash/Spam from Gmail              | `standard`, `drafts`                     | H9                          | 5       | 3       | 2      | 0     |
| `conversation.spec.ts`     | CONV   | expand, fold, scroll, select, quotes, read on open, arrivals, links                          | `standard`                               | H18, H19                    | 14      | 2       | 11     | 1     |
| `triage-actions.spec.ts`   | TRI    | `e`, `Shift+E`, `#`, `!`, `s`, `u`, selection, removal table, recorded ids                   | `standard`, `empty`                      | H8a, H9                     | 11      | 7       | 4      | 0     |
| `undo.spec.ts`             | UND    | the `z` stack, partial revert, per-account history, persistence                              | `standard`                               | H15                         | 8       | 4       | 3      | 1     |
| `labels.spec.ts`           | LAB    | `l`, create, offline create, 409, `y`, labels learned from history                           | `standard`                               | H8a, H9                     | 8       | 2       | 6      | 0     |
| `search.spec.ts`           | SRC    | local, Gmail merge, debounce, offline, query language, paging                                | `standard`, `bulk`                       | H9, H13                     | 7       | 3       | 3      | 1     |
| `splits.spec.ts`           | SPL    | routing, `Tab`, pins, own reply, custom splits, starters, hiding, cap                        | `standard`, `splits`, `bulk`, `bulk1000` | H10, H12, H15               | 10      | 6       | 3      | 1     |
| `snooze.spec.ts`           | SNZ    | picker, snooze, wake triggers, clearing rules, history echo, undo                            | `standard`                               | H13, H14, H15               | 7       | 6       | 0      | 1     |
| `compose-reply.spec.ts`    | REP    | inline composer, addressing, quoting, forward, resume rules, Gmail drafts                    | `standard`, `drafts`                     | H4, H6                      | 12      | 9       | 3      | 0     |
| `compose-new.spec.ts`      | NEW    | `c` screen, formatting, shortcuts, `Esc` machine, untouched rule, discard                    | `standard`                               | H4                          | 12      | 7       | 5      | 0     |
| `recipients.spec.ts`       | RCP    | chips, autocomplete, ranking, invalid addresses, Cc/Bcc                                      | `contacts`, `standard`                   | H23                         | 8       | 3       | 5      | 0     |
| `attachments.spec.ts`      | ATT    | attach, download on demand, offline forward, MIME tree                                       | `standard`                               | H4, H9, H23                 | 4       | 3       | 1      | 0     |
| `drafts.spec.ts`           | DRF    | autosave and upsert, Drafts `Enter`, Gmail-side edits and deletes, offline, reload and crash | `standard`, `drafts`                     | H6, H15, H16                | 9       | 6       | 2      | 1     |
| `undo-send.spec.ts`        | SND    | the outbox state machine, held undo, rejection, no clear answer, restart                     | `standard`                               | H8a, H8b, H9, H13, H15      | 12      | 9       | 3      | 0     |
| `offline.spec.ts`          | OFF    | airplane mode, replay order, restart, conflicts, rejection during replay                     | `standard`                               | H9, H15                     | 6       | 5       | 1      | 0     |
| `gmail-errors.spec.ts`     | ERR    | the outcome matrix for actions and `drafts.create`, back-off, 401, 404                       | `standard`                               | H6, H8, H9, H10, H13        | 9       | 5       | 3      | 1     |
| `sync.spec.ts`             | SYN    | connect, closed-app catch-up, polls, full resync, focus refresh                              | `standard`, `drafts`, `bulk`             | H6, H8c, H10, H12, H13, H15 | 9       | 5       | 2      | 2     |
| `notifications.spec.ts`    | NOT    | when to notify, summary, mute, Dock badge, click                                             | `standard`                               | H17, H18                    | 6       | 5       | 1      | 0     |
| `settings.spec.ts`         | SET    | the sheet, remote images                                                                     | `standard`                               | H15, H20                    | 2       | 1       | 1      | 0     |
| `palette.spec.ts`          | PAL    | the palette, and its commands matching their keys                                            | `standard`                               | –                           | 2       | 1       | 1      | 0     |
| `security.spec.ts`         | SEC    | hostile HTML, markup in metadata, links, no credentials in the webview                       | `security`, `standard`                   | H19, H20, H21               | 4       | 3       | 1      | 0     |
| `ui-consistency.spec.ts`   | UIC    | no flash-back, reload continuity, a failed submit, rows, quit                                | `bulk`, `standard`, `empty`              | H8a, H15, H16, H23          | 5       | 2       | 3      | 0     |
| **Total**                  |        |                                                                                              |                                          |                             | **184** | **107** | **68** | **9** |

**Order to build.** Write all P1 cases first. They cover every Done-when item except performance (section 6) and every data-loss risk the SPEC names. The harness items most P1 cases depend on are H8a (`hold`), H9 (the request log), H13 (the clock), H15 (restart), and H22 (`settle`). Build those first.

---

## 5. Cases

### 5.1 `accounts.spec.ts` (ACC)

#### ACC-01 · P1 · First run adds the first account, and its token goes to the Keychain

_ST · §Accounts, §Security · Done when 1_

- **Seed:** `none`.
- **Steps:** Open the app. Click **Add a Gmail account**. The fake OAuth approves `A`. Call `settle()`.
- **Expect UI:**
  - The first-run screen shows before sign-in, with no rail and no list.
  - After sign-in, A's Inbox shows the same threads as Gmail's `INBOX`.
- **Expect platform:**
  - `keychain()` has exactly one item, `{service: "ubermensch", account: "ana@acme.test"}`.
  - The auth URL in `openedUrls` uses the system browser. It has a `redirect_uri` on `http://127.0.0.1:<port>`, a `code_challenge` with `S256`, and `scope` equal to `https://www.googleapis.com/auth/gmail.modify` only.
  - `inspect.dbContains(<refresh token>)` is false.

#### ACC-02 · P1 · Add a second account from the palette, then switch with `Ctrl+1` and `Ctrl+2`

_EP, BVA · §Accounts, §Switching, §Look (avatars) · Done when 1_

- **Seed:** `standard` with `{connect: ["A"]}`, so `B` exists only in Gmail.
- **Steps:**
  1. Press `Cmd+K`, type "Add account", press `Enter`. The fake approves `B`.
  2. Press `Ctrl+2`, then `Ctrl+1`, then `Ctrl+3`.
- **Expect:**
  - A second avatar appears below A's, in the order the accounts were added. B's avatar initial is **Z**, because B's display name comes from its primary `sendAs` ("Zed Bolt"), not from the address.
  - `Ctrl+2` shows only B's Inbox rows. `Ctrl+1` shows only A's.
  - `Ctrl+3` (out of range) does nothing.
  - `keychain()` has two items.
  - Avatar hues are `#714CB6` and then `#D6478A` (P3 detail; assert only if cheap).

#### ACC-03 · P1 · Switching closes overlays and returns each account to its own place

_ST · §Switching_

- **Seed:** `standard`.
- **Steps:**
  1. On A, press `g` `s` (Starred), then `j` (second row).
  2. Press `Ctrl+2`.
  3. On B, press `j` and `Enter` (open a thread), then `Cmd+K` (palette open).
  4. Press `Ctrl+1`, then `/` and type "venue".
  5. Press `Ctrl+2`.
  6. Repeat one switch by clicking an avatar and one through Command palette → "Switch account".
- **Expect:**
  - On B's first visit since launch, B shows its Inbox with the top row highlighted.
  - Switching to A closes the palette. A is on Starred with the second row highlighted.
  - Switching back to B closes search. B shows its Inbox **list**, with the thread it had open highlighted. The open conversation is not reopened.
  - The avatar click and the palette command behave the same way.

#### ACC-04 · P1 · Switching with an open composer works like a second `Esc`

_DT · §Switching, §Keyboard_

- **Seed:** `standard`. Each row of the table starts on A, makes the change, and presses `Ctrl+2`. Then `Ctrl+1`, `settle()`, and inspect.

| Composer on A                       | Change        | After the switch                                                                                            |
| ----------------------------------- | ------------- | ----------------------------------------------------------------------------------------------------------- |
| fresh `r` on `p1`                   | none          | Discarded: no draft stub in `p1`, no Drafts row, no Gmail draft                                             |
| fresh `r` on `p1`                   | typed body    | Kept in **A**: stub in `p1`, A's Drafts row, Gmail draft in A. B's Drafts and `gmail(B).drafts` stay empty. |
| `c`                                 | typed subject | Kept in A's Drafts                                                                                          |
| resumed draft (from a previous row) | none          | Kept                                                                                                        |

#### ACC-05 · P2 · Avatar unread counts

_BVA, EP · §Switching_

- **Seed:** `standard`. B has 2 unread Inbox threads.
- **Steps:**
  1. Read B's avatar badge.
  2. On B, open b1 and b2.
  3. Back on A, deliver two unread messages to B **in the same new thread**. `poll()`.
  4. Deliver one unread message to B with no `INBOX` label. `poll()`.
- **Expect:**
  - The badge shows 2.
  - After b1 and b2 are read, the badge is hidden (count 0).
  - After step 3, the badge shows 1. Unread threads are counted, not messages.
  - Step 4 does not change the badge.
  - A's avatar always shows A's unread Inbox thread count.

#### ACC-06 · P1 · Accounts off screen keep syncing, notifying, and sending

_EP · §Switching, §Send and undo_

- **Seed:** `standard`.
- **Steps:**
  1. On A, reply to `p2`, type, and press `Cmd+Enter`.
  2. Within 1 s, press `Ctrl+2`.
  3. `deliver` an unread Inbox message to A.
  4. `app.advance(10_000)`, then `settle()`.
- **Expect:**
  - `gmail(A)` has the sent reply in `p2`'s thread.
  - The send toast stayed on screen on B and now says "Sent".
  - `platform().notifications` has one entry naming `ana@acme.test`.
  - A's avatar count went up by 1.
- **Variant:** archive on A while offline, switch to B, go back online. `gmail(A)` shows the archive with B still on screen.

#### ACC-07 · P1 · Remove an account

_DT · §Settings, §Accounts_

| Row | Setup                                                                                                                          | Expect                                                                                                                                                                                                                                                       |
| --- | ------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| a   | Remove `B` while A is on screen. B's queue is empty. Online. Before removing, open an attachment on B so its cache has a file. | The confirmation reads "Remove ben@bolt.test?" with no loss counts. Afterwards: B's avatar is gone, `keychain()` has no B item, `revocations()` has B's refresh token, `inspect.rows(B)` is all zero, and `inspect.cacheFiles(B)` is empty. A is unaffected. |
| b   | Remove `A` while it is on screen, with 2 queued actions (offline: `e` and `s`) and 1 waiting send                              | The confirmation states 2 actions and 1 send will be lost. After confirming: B is on screen. Going online and running `settle()` changes nothing in `gmail(A)`: no archive, no star, no sent message.                                                        |
| c   | Remove the last remaining account                                                                                              | The first-run screen shows.                                                                                                                                                                                                                                  |
| d   | **Cancel** in the confirmation                                                                                                 | Nothing changes.                                                                                                                                                                                                                                             |

- **Then** re-add `A` through Add account. A's Inbox matches Gmail again. `z` does nothing, because the undo history is gone with the old rows.
- **Open question Q8:** revoking while offline.

#### ACC-08 · P1 · `invalid_grant` shows Sign in again; the queue and outbox wait, then recover

_ST · §Accounts, §Gmail errors (401)_

- **Seed:** `standard`.
- **Steps:**
  1. `invalidGrant(A, true)` and `expireAccessTokens(A)`.
  2. On A, press `e` on `p1`. Reply to `p2`, then `Cmd+Enter`.
  3. `app.advance(11_000)`, then `settle()`.
  4. Open `plan`.
  5. Open Settings, then click **Sign in again** on A's block. The fake approves. `invalidGrant(A, false)`, then `settle()`.
- **Expect while signed out:**
  - A's avatar shows the needs-sign-in marker, and its tooltip includes "Sign in again".
  - Settings shows **Sign in again** on A's block only.
  - `plan` opens with its bodies; cached mail is readable.
  - `p1` stays out of the local Inbox. `gmail(A)` `p1` still has `INBOX`. No reply was sent.
  - No error toast appears, because this is not a rejection.
  - B keeps working: `e` on b1 reaches Gmail.
- **Expect after sign-in:**
  - `p1` is archived on Gmail, and the reply is sent once.
  - The marker is gone.

#### ACC-09 · P2 · A failed sign-in adds nothing

_EG · §Accounts_

- **Seed:** `none`.
- **Steps:** `nextSignIn({deny: true})`, then click **Add a Gmail account**. Then `nextSignIn({abandon: true})` and click again. Then click once more with no fault.
- **Expect:**
  - After the deny, the first-run screen shows an error and no account is added. `keychain()` is empty.
  - The abandoned attempt leaves the button usable again.
  - The last attempt succeeds.

#### ACC-10 · P2 · Accounts with the same Gmail thread id stay separate

_EG · §Data_

- **Seed:** `standard`. B's `collide` has the same Gmail thread id as A's `p1`.
- **Steps:** On B, open `collide`. Then `e`. Then `Ctrl+1` and open `p1`.
- **Expect:**
  - B shows "B collide" content.
  - After `e`, `gmail(B)` `collide` is archived.
  - A's `p1` is still in A's Inbox with A's content, and `gmail(A)` `p1` is unchanged.

#### ACC-11 · P1 · A task left from before Sign in again does not sign the account out again

_EG · §Accounts, §Gmail errors (401)_

- **Seed:** `standard`.
- **Steps:**
  1. Fault every `threads.get format=full` with a 500, then `deliver` a new thread, so its body fetch backs off.
  2. `invalidGrant(A, true)`, `expireAccessTokens(A)`, `poll()`: A shows Sign in again.
  3. Sign in again from Settings (the old grant stays revoked), then `clearFaults()`.
  4. `advance(60_000)`, then `deliver` another thread.
- **Expect:** after step 4, A has no Sign in again marker and the new thread is listed: the old runtime's fetch, retrying on the dead grant, signs nothing out.

#### ACC-12 · P2 · Re-adding an account while its removal is still revoking keeps the new sign-in

_EG · §Accounts, §Settings (Remove account)_

- **Seed:** `standard`.
- **Steps:** `hold("oauth.revoke")`. Remove B in Settings and wait until the revoke is held. `nextSignIn({ email: B })`, run **Add account**, `settle()`, then release the revoke.
- **Expect:**
  - B is back. `revocations()` lists only B's old token, and `keychain()` holds a new B token.
  - After `expireAccessTokens(B)`, new mail for B still syncs and B shows no Sign in again marker: the new grant was not revoked.

#### ACC-13 · P1 · Adding an account that is already signed in replays its queue once

_EG · §Accounts, §Offline (replay)_

- **Seed:** `standard`.
- **Steps:** `hold("messages.batchModify")`. `e` on `p1`, and wait until the request is held. `nextSignIn({ email: A })` and run **Add account**; wait until A lists Inbox again (the new runtime runs). Release, then `settle()`.
- **Expect:** `p1` is archived in Gmail with exactly one `batchModify`: the new runtime's queue waits for the old one to finish the op it had in flight.

### 5.2 `inbox-navigation.spec.ts` (NAV)

#### NAV-01 · P1 · Inbox rows show the sender, subject — snippet, time, and an unread mark

_EP · §Window, §Views, §Layout_

- **Seed:** `standard`.
- **Expect:**
  - Each A Inbox thread has exactly one row.
  - Each row shows the sender's display name, the subject, a dash, the snippet, and a right-aligned time, all from the thread's latest message.
  - Unread rows (`plan`, `addr`, `p1`, `p3`, `p5`, `news*`) have the dot and the heavier sender. Read rows do not.
  - On the first visit the top row is highlighted.
  - The list header shows `Important <n> · Other <m>`.
  - The hint bar shows `E to Mark Done · H to snooze · ⌘K for commands` (P3 detail).

#### NAV-02 · P1 · `j` and `k` move the highlight across split boundaries

_BVA · §Window, §Keyboard · Done when 4_

- **Seed:** `standard`. For the scroll check, `bulk`.
- **Steps:**
  1. Press `j` until the last Important row, then `j` once more, then `k`.
  2. In `bulk`, press `j` 45 times.
- **Expect:**
  - One more `j` from the last Important row moves to the **first Other row**. The split header is never highlighted.
  - `k` goes back to the last Important row.
  - `aria-activedescendant` follows the highlight.
  - In `bulk`, the highlighted row is always inside the scroll viewport.
  - Edge behavior at the first and last row is not asserted (Q11).

#### NAV-03 · P1 · `Enter` opens a conversation, `Esc` returns with the highlight; a click opens too

_ST · §Window · Done when 4_

- **Steps:** Highlight the 4th row and press `Enter`. Then press `Esc`. Then click the 5th row.
- **Expect:**
  - The conversation view replaces the list: the list is not in the DOM and the subject heading shows. No second window or side column opens.
  - `Esc` shows the list with the 4th row still highlighted.
  - Clicking the 5th row opens it.

#### NAV-04 · P1 · `j` and `k` inside a conversation open the next and previous conversation

_ST · §Window, §Reading_

- **Seed:** `standard`.
- **Steps:**
  1. Open the 2nd row. Press `j`, then `k` `k`, then `Esc`.
  2. Open the last Important row and press `j`.
- **Expect:**
  - `j` opens the 3rd conversation, which becomes read (its list dot is gone after `Esc`).
  - `k` `k` opens the 1st conversation.
  - `Esc` returns to the list with the 1st row highlighted.
  - From the last Important row, `j` opens the first Other conversation.

#### NAV-05 · P2 · `g` jumps and the rail

_DT · §Views, §Window, §Labels_

| Keys or click                                               | View shown                                  | Header                                            |
| ----------------------------------------------------------- | ------------------------------------------- | ------------------------------------------------- |
| `g` `e`                                                     | Done                                        | "Done"                                            |
| `g` `h`                                                     | Snoozed                                     | "Snoozed"                                         |
| `g` `s`                                                     | Starred                                     | "Starred"                                         |
| `g` `t`                                                     | Sent                                        | "Sent"                                            |
| `g` `d`                                                     | Drafts                                      | "Drafts"                                          |
| `g` `#`                                                     | Trash                                       | "Trash"                                           |
| `g` `!`                                                     | Spam                                        | "Spam"                                            |
| `g` `l`, choose "Work/Clients"                              | that label's view                           | "Work/Clients" (a nested label shows as its path) |
| `g` `i`, from Starred                                       | Inbox, highlight on the first Important row | split names (stays Inbox)                         |
| `g` `o`, from Starred                                       | Inbox, highlight on the first Other row     | split names                                       |
| Each rail icon: Inbox, Done, Snoozed, Starred, Sent, Drafts | the same views                              |                                                   |

#### NAV-06 · P2 · Empty states

_EP · §Window_

- **Seed:** `empty`.
- **Expect:**
  - Inbox shows the quiet inbox-zero state: no rows and no splash.
  - `g` `s` shows the empty state for that view.
  - Archiving the last Inbox row returns to inbox zero (also covered by TRI-02).

#### NAV-07 · P2 · The highlight moves when a poll removes the highlighted row

_ST · §Keyboard ("When a patch from Rust removes the highlighted row…")_

- **Seed:** `standard`.

| Situation                             | Change from another client      | Expect after `poll()`                                                                           |
| ------------------------------------- | ------------------------------- | ----------------------------------------------------------------------------------------------- |
| A middle row is highlighted           | remove `INBOX` from that thread | The highlight is on the row that followed it.                                                   |
| The last row is highlighted           | remove `INBOX` from that thread | The highlight is on the new last row.                                                           |
| Conversation X (a middle row) is open | remove `INBOX` from X           | X stays open with its messages. `j` then opens the conversation that followed X's old position. |

#### NAV-08 · P2 · `j` keeps the prefetched neighbors it still needs

_EG · §Performance (prefetch), §Subscriptions_

- **Seed:** `standard`.
- **Steps:** with `p1` highlighted, `settle()` (the prefetch holds `p1` and `p2`). Press `j`, `settle()`.
- **Expect:** three conversation subscriptions, and `p1` and `p2` keep the subscription ids they had: not dropped (data cleared) and made again.

#### NAV-09 · P2 · `g o` highlights Other however long the Inbox takes to load

_EG · §Views (`g` then `o`)_

- **Seed:** `standard`.
- **Steps:** `g e`, `settle()`. Park the page's submits, press `g o`, run page time 2 s, release, `settle()`.
- **Expect:** the highlight is on the first row of Other, and Other is the active split.

### 5.3 `views.spec.ts` (VIEW)

#### VIEW-01 · P2 · A thread's latest message sets its row

_DT · §Views ("A thread's latest message is…")_

- **Seed:** `standard`, plus in-test `deliver` calls.

| Thread contents, oldest to newest               | View   | The row shows the sender, snippet, and sort time of                    |
| ----------------------------------------------- | ------ | ---------------------------------------------------------------------- |
| `trashedLatest`: m1 Mia (INBOX), m2 Ned (TRASH) | Inbox  | m1, Mia. It sorts by m1's time.                                        |
| m1 Oli (INBOX), m2 a draft by A                 | Inbox  | m1, Oli                                                                |
| m1 Pat and m2 Rex, both `SPAM`                  | Spam   | m2, Rex: no message qualifies, so the newest non-draft message is used |
| Only a draft (`dNew` in `drafts`)               | Drafts | the draft                                                              |

#### VIEW-02 · P1 · View membership comes from message labels

_DT · §Views_

- **Seed:** `standard`, plus threads delivered in the test.

| Thread                           | In Inbox                                  | In Trash | In Spam | In Starred                                    |
| -------------------------------- | ----------------------------------------- | -------- | ------- | --------------------------------------------- |
| m1 `INBOX`, m2 `TRASH`           | yes                                       | yes      | –       | –                                             |
| m1 `INBOX`, m2 `STARRED`+`TRASH` | yes                                       | yes      | –       | **no**: the star is only on a trashed message |
| m1 `STARRED`, m2 `INBOX`+`SPAM`  | **no**: `INBOX` is only on a spam message | –        | yes     | yes                                           |

#### VIEW-03 · P1 · Done holds threads seen in Inbox that left it

_DT · §Views (Done, `seen_in_inbox`)_

- **Seed:** `standard`.

| Case                                                                | In Done?                                       |
| ------------------------------------------------------------------- | ---------------------------------------------- |
| `e` on `p1` in this app                                             | yes                                            |
| `archivedOld` (left Inbox before the first connect)                 | no                                             |
| `p2`, seen in Inbox, then archived by another client, then `poll()` | yes                                            |
| `p3` snoozed with `h`                                               | no. It is in Snoozed.                          |
| `p4` archived, then `#` from Done                                   | no. It is in Trash: every message is in trash. |
| `p5` archived, then `Shift+E` from Done                             | no. It is back in Inbox.                       |

#### VIEW-04 · P1 · Drafts lists every draft, and mail actions do nothing there

_DT · §Views (Drafts), §UI state (Subscriptions: Drafts keyed by draft id)_

- **Seed:** `drafts`.
- **Expect, listing:**
  - Drafts has one row per draft: `dTwoA` and `dTwoB` are two rows for the same thread.
  - A draft composed while offline is listed before it reaches Gmail.
  - Sending a draft removes its row at once. The row does not return while the send waits in the outbox. If the send is undone, the row comes back.
  - Discarding a draft removes its row.
- **Expect, mail actions (P2 part):** in Drafts, `e`, `#`, `!`, `s`, `u`, `y`, `h`, and `l` change nothing. No row changes, and `req(messages.batchModify)` stays empty.

#### VIEW-05 · P2 · Label, Trash, and Spam views also list from Gmail

_EP · §Views ("Opening a Label, Trash, or Spam view while online also runs `threads.list`")_

- **Seed:** `standard`. `projOld*`, `trashOld`, and `spamOld` are not fetched by the connect sync.
- **Steps:** `g` `l` → Projects. Then `g` `#`. Then `g` `!`. Then open one fetched thread. Then `offline(true)` and open Projects again.
- **Expect:**
  - Projects lists `projOld1`…`projOld3` once their metadata arrives. `req(threads.list)` has `labelIds=<Projects id>` and `maxResults=15`.
  - Trash and Spam list their old threads. Those requests carry `includeSpamTrash=true`.
  - The opened thread shows its body, fetched on open.
  - Offline, Projects shows the cached rows and no error.

### 5.4 `conversation.spec.ts` (CONV)

Decision table used by CONV-01, CONV-02, and CONV-03 (§Reading):

| Latest message? | Unread when opened? | Opens     |
| --------------- | ------------------- | --------- |
| yes             | any                 | expanded  |
| no              | yes                 | expanded  |
| no              | no                  | collapsed |

- **Folding:** a run of collapsed messages of length L. If L ≤ 4, every message shows. If L ≥ 5, the run shows its first message, an "L−2 older messages" line, and its last message.
- **Scroll:** to the first unread message, or else to the latest.
- **Selection:** always starts on the latest message.

#### CONV-01 · P1 · A long thread opens with read messages collapsed and folded, scrolled to the first unread

_DT, ST · §Reading · Done when 7_

- **Seed:** `standard`. Open `plan`.
- **Expect:**
  - Messages are oldest first.
  - m1 is a collapsed line with sender, snippet, and time. Then a "7 older messages" fold. Then m9, collapsed.
  - m10, m11, and m12 are expanded, each in an `iframe[data-frame]`. Collapsed messages have no iframe.
  - The view is scrolled so m10 is at the top of the viewport and m1 is above it.
  - The selection bar is on **m12**, not m10.
  - After `settle()`: the list row has no unread dot, A's avatar count went down by 1, and `gmail(A)` has no `UNREAD` on any `plan` message.

#### CONV-02 · P2 · Fold boundary: four collapsed messages show, five fold

_BVA · §Reading_

- **Seed:** `standard`. Open `fold`.
- **Expect:**
  - m1–m4 show as four collapsed lines, with no fold.
  - m5 is expanded.
  - Then m6, a "3 older messages" fold, and m10.
  - m11 is expanded.
  - The view is scrolled to m5, and m11 is selected.
- **Variant:** open the all-read 3-message `own` thread. Only the latest message is expanded, and the view is scrolled to it.

#### CONV-03 · P2 · Expansion is fixed when the conversation opens

_ST · §UI state (Subscriptions: "Later read changes … do not collapse them")_

- **Steps:**
  1. Open `plan` and wait for read-on-open to commit (`settle()`).
  2. Press `p`, then `Esc`, then `Enter` on `plan` again.
- **Expect:**
  - After the read commits, m10 and m11 stay expanded.
  - After reopening, only m12 is expanded. The collapsed run m1–m11 shows as m1, "9 older messages", and m11.
  - The view is scrolled to m12.
  - The selection is back on m12, even though `p` had moved it before the close.

#### CONV-04 · P2 · Expanding, collapsing, and selecting with the mouse

_EP · §Reading_

- **Steps:** In `plan`, click m1's collapsed line, then click m1's header. Click the fold. Click inside m11's body.
- **Expect:**
  - Clicking m1's collapsed line expands it and selects it.
  - Clicking its header collapses it.
  - Clicking the fold shows each folded message as a collapsed line.
  - Clicking inside m11's body selects m11.

#### CONV-05 · P2 · `n`, `p`, and `o` select and expand messages

_ST · §Reading_

- **Seed:** `standard`. In `plan`, first make a reply draft: `r`, type "x", `Esc` `Esc`. Then reopen `plan`.
- **Steps:**
  1. Press `p`, `p`, `p`, `p`.
  2. Press `o`, then `o`.
  3. Press `n` until m12, then `n` again.
- **Expect:**
  - The first `p` selects m11, then m10, then m9.
  - The fourth `p` selects m8, which is inside the fold. The fold opens, m8 is selected, and m8 is scrolled into view.
  - `o` expands m8. The second `o` collapses it.
  - `n` stops on m12. The extra `n` does not select the draft stub, and the selection stays on m12.

#### CONV-06 · P2 · `Shift+O` expands everything; pressing it again restores the default

_ST · §Reading_

- **Seed:** `standard`. Open `fwd`, then `plan`.
- **Expect:**
  - In `fwd`, `Shift+O` shows the quoted history in the iframe.
  - In `plan`, `Shift+O` expands all 12 messages (12 iframes).
  - Pressing `Shift+O` again returns `plan` exactly to CONV-01's layout.

#### CONV-07 · P2 · Quoted history sits behind `•••`

_EP · §Reading_

- **Seed:** `standard`. Open `fwd`.
- **Expect:** the quoted history is hidden. Clicking `•••` in the iframe shows it. Clicking again hides it.

#### CONV-08 · P1 · Opening marks the thread read locally and on Gmail

_ST · §Reading_

- **Seed:** `standard`. `p1` is unread. Let `n` be the Dock badge from `platform().badge`.
- **Steps:**
  1. Open `p1`, press `Esc`, then `settle()`.
  2. Variant: `offline(true)`, open `p3`, then `offline(false)` and `settle()`.
- **Expect:**
  - `p1`'s row has no dot. A's avatar count and `badge()` both went down by 1. `gmail(A)` `p1` has no `UNREAD`.
  - In the offline variant, `p3` is read locally at once and on Gmail after reconnecting.
  - The read change is not an undo entry (see UND-03).

#### CONV-09 · P2 · A message that arrives while the conversation is open

_DT · §Reading, §UI state (Subscriptions)_

- **Seed:** `standard`. Open `p2`, then `deliver` a reply from Carol into `p2`'s thread and `poll()`.

| Selection when it arrives         | Window                           | Expect                                                                                                                                                              |
| --------------------------------- | -------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| on the latest message             | focused                          | The new message is expanded and **selected**. It is marked read locally and on Gmail. It stays expanded. A `z` after this does not make it unread. No notification. |
| on an earlier message (`p` first) | focused                          | The new message is expanded, and the selection **stays** where it was.                                                                                              |
| on the latest message             | unfocused (`gmail.focus(false)`) | The new message is expanded and selected. It stays unread: `gmail(A)` keeps `UNREAD`. A notification is raised (see NOT-02).                                        |

#### CONV-10 · P2 · If the selected message leaves, the latest is selected

_EG · §Reading_

- **Steps:** In `plan`, press `p` to select m11. `other(modify m11 +TRASH)`, then `poll()`.
- **Expect:** m11 is no longer listed, and m12 is selected.

#### CONV-11 · P2 · Messages in trash or spam are hidden outside Trash and Spam

_DT · §Reading_

- **Seed:** `standard`.

| Thread                      | Opened from | Listed                                                                                   |
| --------------------------- | ----------- | ---------------------------------------------------------------------------------------- |
| `trashedLatest`             | Inbox       | m1 only                                                                                  |
| `trashedLatest`             | Trash       | m1 and m2                                                                                |
| A thread with a reply draft | anywhere    | The draft is never one of the messages. It shows as a collapsed draft stub (see REP-07). |

#### CONV-12 · P2 · `Tab` cycles links, `Enter` opens one outside the app, `Esc` clears it

_ST · §Reading, §Keyboard_

- **Seed:** `standard`. Open `links`.
- **Steps:** `Tab`, `Tab`, `Shift+Tab`, `Enter`. Then `Esc`, then `Esc`.
- **Expect:**
  - The first `Tab` outlines link 1, the second link 2, and `Shift+Tab` link 1 again.
  - `Enter` records `https://example.org/a` as opened (H19). The app's URL is unchanged.
  - The first `Esc` clears the link focus, and the conversation stays open.
  - The second `Esc` returns to the list.

#### CONV-13 · P2 · Shortcuts still work with focus inside a message body

_EG · §Reading ("forwards keystrokes from the iframe")_

- **Seed:** `standard`.
- **Steps:** Open `p4`. Click inside its body so the iframe has focus. Press `?`, then `Esc`, click the body again, and press `e`.
- **Expect:**
  - `?` opens the shortcut sheet.
  - `e` archives `p4`, and the next conversation opens.

#### CONV-14 · P3 · Message iframes size to their content

_EP · §Reading_

- **Seed:** `standard`. Open `fwd` with its quoted history expanded.
- **Expect:**
  - Each iframe's height is at least its document's `scrollHeight`.
  - No iframe scrolls internally. The conversation container is the only scroller.

### 5.5 `triage-actions.spec.ts` (TRI)

Action table. It comes from §Archive, trash, star, read and from the removal table in §UI state (Actions). The Gmail change applies to the thread's **non-draft** messages at the time of the action.

| Key       | Gmail change                                                                               | The row leaves at once (UI)                       | The thread ends up in |
| --------- | ------------------------------------------------------------------------------------------ | ------------------------------------------------- | --------------------- |
| `e`       | −`INBOX`. A snoozed thread also loses its snooze.                                          | Inbox, Snoozed                                    | Done                  |
| `Shift+E` | +`INBOX`. In Trash also −`TRASH`; in Spam also −`SPAM`. A snoozed thread loses its snooze. | Done, Snoozed, Trash, Spam                        | Inbox                 |
| `#`       | +`TRASH` −`INBOX` −`SPAM`                                                                  | every list except Trash                           | Trash                 |
| `!`       | +`SPAM` −`INBOX` −`TRASH`                                                                  | every list except Spam                            | Spam                  |
| `s`       | ±`STARRED`                                                                                 | Starred (when unstarring)                         | Starred               |
| `u`       | +`UNREAD` on the latest message only (DEVIATIONS), or −`UNREAD` on every message           | –                                                 | –                     |
| `h`       | −`INBOX`, and `snooze_until` set locally                                                   | Inbox, Done                                       | Snoozed               |
| `l`       | ±user label                                                                                | the current label view (when removing that label) | the label view        |
| `y`       | −the current view's label                                                                  | the current view                                  | –                     |

#### TRI-01 · P1 · `e` on the list removes the row before Gmail answers, and moves the highlight

_ST, BVA · §Archive, trash, star, read; §Keyboard · Done when 4, 6_

- **Seed:** `standard`.
- **Steps:**
  1. `hold(messages.batchModify)`. Highlight the 3rd Inbox row (`j` `j`) and press `e`.
  2. `release`, then `settle()`.
  3. Press `j` until the last row, then `e`.
- **Expect:**
  - While the request is held, the 3rd row is gone and the former 4th row sits in 3rd place, highlighted.
  - After release, `gmail(A)` shows the thread's messages without `INBOX`. The body of `req(messages.batchModify)` lists exactly the thread's message ids. The thread is listed in Done.
  - Archiving the last row moves the highlight to the new last row.

#### TRI-02 · P1 · `e` in an open conversation opens the next one, or the previous one, or returns to the empty list

_BVA · §Keyboard ("When the acted-on conversation leaves the current list")_

- **Seed:** `empty`, then deliver three threads X, Y, Z (top to bottom).
- **Steps:** Open Y and press `e`. Then `e` again. Then `e` again.
- **Expect:**
  - The first `e` opens Z, the next one.
  - The second `e`, on the last conversation, opens X, the previous one.
  - The third `e`, with none left, returns to the inbox-zero list.
  - All three threads are archived in Gmail.

#### TRI-03 · P1 · `#` moves to Trash and never touches draft messages

_DT · §Archive, trash, star, read · Done when 6_

- **Seed:** `standard`. On `p4`, make a reply draft first: `r`, type, `Esc` `Esc`, `settle()`.
- **Steps:**
  1. `hold(batchModify)`, press `#` on `p4`, then `release`.
  2. `g` `s`, then `#` on `starredOld`.
  3. `g` `!`, then `#` on `spamOld`.
- **Expect:**
  - In step 1, the row is gone while the request is held. Afterwards, `p4`'s non-draft messages have `TRASH` and no `INBOX`. The Gmail draft still exists, and its message id is not in the `batchModify` body.
  - In step 2, the row leaves Starred.
  - In step 3, `spamOld` loses `SPAM` and gains `TRASH`.
  - Both `p4` and `starredOld` are listed in Trash.

#### TRI-04 · P2 · `!` moves to Spam

_DT · §Archive, trash, star, read_

- **Steps:** `!` on `p5` in Inbox. Then `!` on `trashOld` in Trash.
- **Expect:**
  - `p5` gains `SPAM` and loses `INBOX`. It is listed in Spam.
  - `trashOld` loses `TRASH`, gains `SPAM`, and leaves Trash.

#### TRI-05 · P1 · Star and read toggles

_ST · §Archive, trash, star, read; DEVIATIONS (Gmail) · Done when 6_

- **Seed:** `standard`. Use `own`, a read thread with 2 messages.
- **Steps:**
  1. `hold(batchModify)`, press `s`, then `release`.
  2. Press `s` again.
  3. Star `p2`, go to Starred (`g` `s`), and press `s` on `p2` there.
  4. On `own`, press `u`, then `u` again.
- **Expect:**
  - In step 1, the star shows while the request is held. Afterwards, `own` is starred in Gmail and listed in Starred.
  - In step 2, the star is removed in Gmail.
  - In step 3, `p2`'s row leaves Starred at once.
  - The first `u` shows the unread dot. In Gmail only the latest message gets `UNREAD`.
  - The second `u` removes the dot. In Gmail no message has `UNREAD`.

#### TRI-06 · P1 · `x` and `Shift+J` / `Shift+K` select; actions apply to the whole selection as one undo entry

_ST · §Keyboard, §Undo_

- **Seed:** `standard`.
- **Steps:**
  1. Highlight row 2, press `x`, then `Shift+J` twice. Press `e`. Then `z`.
  2. Press `x` on row 2, then plain `j`, then `e`.
- **Expect:**
  - Rows 2–4 show the selected style.
  - `e` removes all three rows at once, and all three are archived in Gmail.
  - One `z` brings all three back to their positions, in Inbox locally and in Gmail.
  - In step 2, the plain `j` replaces the selection with the row it lands on. `e` archives only that row (row 3); row 2 stays.

#### TRI-07 · P2 · `s` and `u` on a mixed selection follow the `l` rule

_DT · §Keyboard_

| Selection              | Key | Result, locally and in Gmail |
| ---------------------- | --- | ---------------------------- |
| 3 threads, all starred | `s` | all unstarred                |
| 3 threads, 1 starred   | `s` | all starred                  |
| 3 threads, all unread  | `u` | all read                     |
| 3 threads, 1 unread    | `u` | all unread                   |

#### TRI-08 · P1 · `Shift+E` returns to Inbox from any view

_DT · §Archive, trash, star, read; §Snooze_

| Starting view | Thread               | Gmail change      | Expect                                                  |
| ------------- | -------------------- | ----------------- | ------------------------------------------------------- |
| Done          | `p1`, archived first | +`INBOX`          | It leaves Done and is in Inbox.                         |
| Trash         | `trashOld`           | +`INBOX` −`TRASH` | It leaves Trash and is in Inbox.                        |
| Spam          | `spamOld`            | +`INBOX` −`SPAM`  | It leaves Spam and is in Inbox.                         |
| Snoozed       | `p2`, snoozed first  | +`INBOX`          | Its snooze is cleared, and it is in Inbox (see SNZ-07). |

#### TRI-09 · P2 · `Esc` closes the open overlay before it clears a selection

_DT · §Keyboard ("`Esc` clears a selection only when …")_

- **Seed:** `standard`. Select 2 rows with `x` before each row of the table.

| Open when `Esc` is pressed | First `Esc`                             | Second `Esc`         |
| -------------------------- | --------------------------------------- | -------------------- |
| palette (`Cmd+K`)          | closes the palette; the selection stays | clears the selection |
| snooze picker (`h`)        | closes the picker; the selection stays  | clears the selection |
| nothing                    | clears the selection                    | –                    |

#### TRI-10 · P2 · Each action removes its row from the right lists

_DT · §UI state (Actions removal table)_

- **Seed:** `standard`. Parametrize the cases below. Hold `batchModify` in each so only the UI's own removal is observed.

| Action, in view                                         | Row                                                          |
| ------------------------------------------------------- | ------------------------------------------------------------ |
| `e` in Snoozed                                          | leaves                                                       |
| `e` in Starred                                          | **stays**: not in the table, and the thread is still starred |
| `Shift+E` in Done, Snoozed, Trash, Spam                 | leaves                                                       |
| `h` in Inbox, Done                                      | leaves                                                       |
| `#` in Starred, Sent, a label view, Done                | leaves                                                       |
| `!` in Starred, Trash                                   | leaves                                                       |
| unstar (`s`) in Starred                                 | leaves                                                       |
| `y`, or `l` removing the current label, in a label view | leaves                                                       |
| send or discard in Drafts                               | leaves                                                       |

#### TRI-11 · P1 · An action changes only the messages it recorded; a reply arriving meanwhile stays in Inbox

_EG · §Archive, trash, star, read ("A reply that arrives while an archive is queued still lands in the inbox")_

- **Seed:** `standard`.
- **Steps:**
  1. `hold(batchModify)`. Press `e` on `p1`; its only message is m1.
  2. `deliver` a reply m2 from Carol into `p1`'s thread, with `INBOX` and `UNREAD`. `poll()`.
  3. `release`, then `settle()`.
- **Expect:**
  - The `batchModify` body ids are exactly `[m1]`.
  - In Gmail, m1 has no `INBOX` and m2 still has `INBOX`.
  - Locally, `p1` is back in Inbox, unread, with m2 as its latest message.

### 5.6 `undo.spec.ts` (UND)

Which actions make an undo entry (§Undo, §Settings, §Reading):

| Action                                                                                | Undo entry?      |
| ------------------------------------------------------------------------------------- | ---------------- |
| `e`, `Shift+E`, `h`, `s`, `u`, `#`, `!`, `l`, `y`, on one thread or a selection       | one entry each   |
| send                                                                                  | yes (see SND)    |
| pin: Add to split, Move to Important, Move to Other                                   | yes (see SPL-04) |
| mark read on open; marking read a message that arrives while the conversation is open | no               |
| settings changes: remote images, mute                                                 | no               |
| draft save or close, search, view switch                                              | no               |

#### UND-01 · P1 · `z` walks back through the stack

_ST · §Undo, §UI state (Actions)_

- **Seed:** `standard`.
- **Steps:**
  1. Press `z` with an empty history.
  2. Press `e` on `p1`, `#` on `p2`, and `s` on `p3`.
  3. Press `z`, `z`, `z`, then `z` once more.
- **Expect:**
  - Step 1 changes nothing. `req(batchModify)` is empty.
  - The first `z` unstars `p3`.
  - The second `z` brings `p2` back to Inbox and out of Trash.
  - The third `z` brings `p1` back to Inbox.
  - The last `z` does nothing.
  - Each returned row goes back to its old position, after the row that preceded it.
  - After `settle()`, Gmail labels for `p1`–`p3` equal the seed.

#### UND-02 · P1 · Undo reverts only the labels the action changed

_EG · §Undo ("Undo reverts only those, so later changes … stay")_

- **Seed:** `standard`.
- **Steps:**
  1. Press `e` on `p1`, which is unread.
  2. `other(modify p1 −UNREAD +STARRED)`, then `poll()`.
  3. Press `z`.
- **Expect:**
  - `p1` is back in Inbox, still read and starred, both locally and in Gmail.
- **Second partition:** press `s` on `p2`, then `other(modify p2 +Travel)`, `poll()`, `z`. `p2` loses `STARRED` but keeps Travel.

#### UND-03 · P1 · Actions that make no entry are skipped by `z`

_DT · §Undo, §Settings_

- **Steps:**
  1. Press `e` on `p1`.
  2. Open `p3` (unread), then press `Esc`.
  3. Turn remote images off in Settings, and run Command palette → "Mute notifications".
  4. Press `z`, then `z` again.
- **Expect:**
  - The first `z` undoes the `e`: `p1` is back.
  - `p3` stays read.
  - Remote images stay off, and A stays muted.
  - The second `z` does nothing.

#### UND-04 · P1 · Each account has its own undo history

_EP · §Undo_

- **Steps:**
  1. On A, press `e` on `p1`.
  2. Press `Ctrl+2`, then `z`.
  3. On B, press `e` on `b1`.
  4. Press `Ctrl+1`, then `z`.
  5. Press `Ctrl+2`, then `z`.
- **Expect:**
  - In step 2, B is unchanged and A's `p1` stays archived in Gmail.
  - In step 4, A's `p1` comes back.
  - In step 5, `b1` comes back.

#### UND-05 · P2 · A quick `e` followed by `z` `z` undoes the right entries

_ST · §UI state (Actions: "a quick `e` then `z` undoes that `e`")_

- **Steps:** Press `e` on `p1` and `settle()`. Then, in one `keyboard.type` with no waits, press `e` on `p2`, `z`, `z`.
- **Expect:**
  - Both `p2` and `p1` are back in Inbox.
  - Nothing else was undone.
  - Gmail is consistent with the UI after `settle()`.

#### UND-06 · P2 · `z` still works after a restart

_ST · §Undo ("kept in SQLite, so `z` survives a restart")_

- **Steps:** Press `e` on `p1`, `settle()`, `restart()`, then `z`.
- **Expect:** `p1` is back in Inbox, locally and in Gmail.

#### UND-07 · P3 · Only the last 50 entries are kept

_BVA · §Undo_

- **Steps:** Make 51 entries by alternating `s` on `p1` and `p2`. Then press `z` 51 times.
- **Expect:**
  - The first 50 `z` presses each revert one entry.
  - The 51st does nothing.
  - The star state left over from the oldest entry remains.

#### UND-08 · P2 · Undoing a trash does not bring back a reply whose send was undone

_EG · §Undo_

- **Steps:**
  1. Reply to `addr` and send. `app.advance(10_000)`, `settle()`. The toast says Sent.
  2. Click **Undo** on the toast. The sent reply is trashed.
  3. Press `#` on `addr`, then `z`.
- **Expect:**
  - `addr` is back in Inbox.
  - The undone reply is still `TRASH` in Gmail and is not shown in the conversation.

### 5.7 `labels.spec.ts` (LAB)

#### LAB-01 · P1 · `l` adds and removes a user label

_ST · §Labels · Done when 6_

- **Seed:** `standard`.
- **Steps:**
  1. Press `l` on `p1`.
  2. `hold(batchModify)`. Type "Tra" and press `Enter`. `release`, `settle()`.
  3. Press `l` again, choose Travel, then `settle()`.
- **Expect:**
  - The picker lists only user labels: Receipts, Travel, Projects, and "Work/Clients". It shows no `INBOX`, `STARRED`, or `CATEGORY_*`.
  - While the request is held, `p1` already shows in the Travel view (`g` `l` → Travel).
  - After release, `p1` has Travel in Gmail.
  - In step 3, the picker shows Travel with a ✓. After choosing it, Travel is removed from every `p1` message in Gmail.

#### LAB-02 · P2 · `l` on a selection: remove only if every thread has the label

_DT · §Labels_

| Selection                  | Choose Travel | Result             |
| -------------------------- | ------------- | ------------------ |
| 3 threads, all with Travel | yes           | removed from all 3 |
| 3 threads, 1 with Travel   | yes           | added to all 3     |

- **Then** one `z` restores each thread's own earlier state: in the second row, only one thread has Travel again.

#### LAB-03 · P2 · Create a label from the picker and apply it in one action

_ST · §Labels, §UI state ("A label create and the label adds made with it go in one action")_

- **Steps:** Select `p1` and `p2` with `x`. Press `l`, type "Invoices 2026", and choose `Create label "Invoices 2026"`. Then `settle()`.
- **Expect:**
  - Both rows carry the label at once.
  - Gmail has exactly one label named "Invoices 2026", and both threads carry its id.
  - `g` `l` lists the new label.
  - `z` removes the label from both threads. Whether the label itself is deleted is not asserted.

#### LAB-04 · P1 · A label created offline is mapped to its Gmail id when the queue replays

_ST · §Offline ("A label created offline gets a local id, and replay maps it to the Gmail id")_

- **Steps:**
  1. `offline(true)`. Create "Offline L" on `p3` from the picker. Add Travel to `p4`.
  2. Check that the "Offline L" view lists `p3`.
  3. `offline(false)`, then `settle()`.
- **Expect:**
  - Gmail has one "Offline L" label, and `p3`'s messages carry its Gmail id.
  - In the request log, `labels.create` comes before the `batchModify` that uses the label.
  - The "Offline L" view still lists `p3`.

#### LAB-05 · P2 · If `labels.create` says the name exists, the existing label is used

_EG · §Labels, §Gmail errors (409)_

- **Steps:** `gmail.label(A, "Clients")`, as created by another client; the app has not refreshed labels. Then press `l` on `p5` and create "Clients".
- **Expect:**
  - No error toast.
  - Gmail still has exactly one "Clients" label, and `p5` carries its id.
  - The picker then shows "Clients" once.

#### LAB-06 · P2 · `y` removes the current view's label, where it can

_DT · §Labels_

| View                                | `y` on a row     | Gmail change     |
| ----------------------------------- | ---------------- | ---------------- |
| Label: Travel                       | The row leaves.  | −Travel          |
| Inbox                               | The row leaves.  | −`INBOX`         |
| Starred                             | The row leaves.  | −`STARRED`       |
| Trash                               | The row leaves.  | −`TRASH`         |
| Spam                                | The row leaves.  | −`SPAM`          |
| Sent, Drafts, Done, Snoozed, Search | Nothing happens. | no `batchModify` |

#### LAB-07 · P2 · A label first seen in history is picked up

_EG · §Sync ("Refresh `labels.list` … whenever history names a label id the app does not know")_

- **Steps:** `gmail.label(A, "Legal")`, then `other(modify p2 +Legal)`, then `poll()`.
- **Expect:**
  - `g` `l` lists Legal, and its view shows `p2`.
  - The `l` picker on `p2` shows Legal with a ✓.

#### LAB-08 · P2 · A label list read before a create landed does not delete the new label

_EG · §Sync (labels refresh), §Labels_

- **Steps:** `holdAnswer` on the next `labels.list`, then `focus(false)` and `focus(true)` so a refresh reads the labels. Create "Fresh L" on `p1` from the `l` picker and `settle()`. Then release the list.
- **Expect:**
  - `g` `l` still lists Fresh L, and its view shows `p1`.
  - Applying Fresh L to `p2` reaches Gmail with the label's Gmail id. Gmail has one Fresh L.

### 5.8 `search.spec.ts` (SRC)

#### SRC-01 · P1 · `/` shows local results at once; focus moves to results; `Esc` restores the previous view

_ST · §Search · Done when 4_

- **Seed:** `standard`. `hold(threads.list)`, so Gmail search never answers.
- **Steps:**
  1. From Starred with row 2 highlighted, press `/`.
  2. Type "venue e#j!".
  3. Clear the field and type "venue". Press `↓`.
  4. Press `j`, `k`, `s`.
  5. Press `Esc`.
  6. Repeat with `Enter` instead of `↓`, and with `Esc` pressed while focus is still in the field.
- **Expect:**
  - The field has focus.
  - Every typed character goes into the field. Nothing is archived or trashed, and `req(batchModify)` is empty.
  - The `addr` row appears without waiting for Gmail.
  - After `↓` the results have focus, and `j`, `k`, and `s` act on the results: `addr` is starred.
  - `Esc` shows Starred again.
  - The `Enter` and field-`Esc` variants behave the same way.

#### SRC-02 · P2 · Local query language, and what is sent to Gmail

_DT · §Search_

- **Seed:** `standard`.

| Query                                  | Local results include                                | Local results exclude                                                         |
| -------------------------------------- | ---------------------------------------------------- | ----------------------------------------------------------------------------- |
| `quokka` (a word only in `p3`'s body)  | `p3`                                                 | –                                                                             |
| `from:carol`                           | `plan`, `addr`                                       | `p1`                                                                          |
| `subject:venue OR subject:ticket`      | `addr`, `replyto`                                    | –                                                                             |
| `label:receipts`                       | `receipt`                                            | –                                                                             |
| `is:list`                              | `news1`–`news3`                                      | `promo`                                                                       |
| `has:attachment venue`                 | `addr` (the unsupported operator is ignored locally) | –                                                                             |
| a word that appears only in `trashOld` | –                                                    | `trashOld`: local search skips threads with no message outside trash and spam |

- **Then** search `is:list digest`, wait 300 ms, and check that the `q` of `req(threads.list)` is `digest`. `is:list` is stripped before the query goes to Gmail.

#### SRC-03 · P1 · Gmail hits are merged in as their metadata arrives

_ST · §Search_

- **Seed:** `standard`. `invoiceOld` is not cached.
- **Steps:**
  1. Type `/` "invoice 2019". `app.advance(300)`, then `settle()`.
  2. Open the hit.
  3. Search "venue". `app.advance(300)`, then `settle()`.
- **Expect:**
  - Before the Gmail answer, there are no local results.
  - After it, one `invoiceOld` row with sender, subject, snippet, and time.
  - Opening the hit fetches its body at once (`threads.get` with `format=full`) and renders it.
  - The "venue" search triggers no `threads.get` for `addr`, because it is cached.

#### SRC-04 · P2 · The Gmail query waits for a 300 ms pause

_BVA · §Search_

- **Steps:**
  1. Type "inv". `app.advance(299)`.
  2. Type "o". `app.advance(300)`.
  3. Type "ice". `app.advance(300)`.
- **Expect:**
  - After step 1, there is no `threads.list` call.
  - After step 2, exactly one call, with `q=invo`. There is none with `q=inv`.
  - After step 3, one more call with `q=invoice`. Hits from `invo` that are still fetching are not shown under `invoice`.

#### SRC-05 · P1 · Offline search shows cached mail and says the results are from this Mac

_EP · §Search, §Offline · Done when 11_

- **Steps:** `offline(true)`. Press `/` and type "venue".
- **Expect:**
  - The `addr` row shows.
  - The field shows "Results from this Mac".
  - The request log has no Gmail request.

#### SRC-06 · P2 · Actions on search results follow the removal table

_DT · §UI state (Actions)_

| Key on a result | The result row                                  |
| --------------- | ----------------------------------------------- |
| `e`             | stays: the thread still matches                 |
| `#`             | leaves: local search skips trashed-only threads |
| `s`             | stays, now starred                              |
| `y`             | nothing happens, and no request is made         |

#### SRC-07 · P3 · Gmail results come 15 at a time; the next page loads near the oldest Gmail hit

_BVA · §Search, §Views_

- **Seed:** `bulk`. Run two entry points: search for "invoice", and open the Archive2019 label view.
- **Expect:**
  - The first `threads.list` has `maxResults=15`.
  - Scrolling toward the oldest Gmail hit requests the next `pageToken` before the list end is reached.
  - Eventually 40 rows show, with no duplicates.

### 5.9 `splits.spec.ts` (SPL)

Routing decision table. The first match wins (§Where a conversation lands). Rules and pins read the **latest message from someone else**.

| #   | Condition                                                                                                                              | Split            |
| --- | -------------------------------------------------------------------------------------------------------------------------------------- | ---------------- |
| 1   | A sender pin to an enabled split                                                                                                       | the pinned split |
| 2   | A domain pin to an enabled split                                                                                                       | the pinned split |
| 3   | A label pin to an enabled split, on one of the thread's user labels. If several match, the split that comes first in split order wins. | the pinned split |
| 4   | An enabled custom split whose query matches, top to bottom                                                                             | that split       |
| 5   | Important is enabled, and the message is person-to-person: not list mail, and `CATEGORY_PERSONAL` or no category                       | Important        |
| 6   | anything else                                                                                                                          | Other            |

#### SPL-01 · P1 · Default routing to Important and Other, with counts in the header

_DT · §Split Inbox · Done when 3_

- **Seed:** `standard`.
- **Expect:**
  - Important holds `plan`, `fold`, `addr`, `replyto`, `alias`, `own`, `ownAlias`, `fwd`, `links`, `trashedLatest`, and `p1`–`p6`.
  - Other holds `news1`–`news3`, `promo`, `social`, `forum`, and `receipt`. `news2` is there even though it carries `CATEGORY_PERSONAL`, because it has `List-Id`.
  - Each thread appears in exactly one section.
  - The header reads `Important 16 · Other 7`, and the counts equal the section sizes.
  - Each section header shows its name and count.
  - The name of the split that holds the highlight is the active one: it changes when `j` crosses into Other.

#### SPL-02 · P1 · `Tab` and `Shift+Tab` move between visible splits

_BVA · §Split Inbox · Done when 3_

- **Seed:** `splits`. Add a custom split "VIP" with the query `from:carol` (through the settings sheet) so that three splits are visible.
- **Steps:**
  1. Start on the 2nd row of VIP (`plan` and `addr` are in it). Press `Tab`, `Tab`, `Tab`.
  2. Press `Shift+Tab`, `Shift+Tab`, `Shift+Tab`.
  3. Empty VIP (archive its threads) and press `Tab` from Important.
- **Expect:**
  - The first `Tab` highlights the first Important row, with its header scrolled into view.
  - The second `Tab` highlights the first Other row.
  - The third `Tab`, from the last split, does nothing.
  - `Shift+Tab` goes to the first Important row, then the first VIP row, then does nothing on the first split.
  - In step 3, a hidden empty split is skipped.

#### SPL-03 · P1 · Pin precedence: sender over domain over label

_DT · §Where a conversation lands_

- **Seed:** `splits`. Through the sheet, add the custom splits VIP and Team, in that order, each with a query that matches nothing (`from:nobody@none.test`). Each step uses Command palette → "Add to split" on `X` unless noted.

| Step | Pin or setting change                                           | Where `X` lands                                                                      | Where `Y` lands |
| ---- | --------------------------------------------------------------- | ------------------------------------------------------------------------------------ | --------------- |
| 1    | label Proj → Team                                               | Team. The label is only on m1, but label pins read the thread's user labels.         | Important       |
| 2    | domain `corp.test` → VIP                                        | VIP: domain beats label                                                              | VIP             |
| 3    | sender `x@corp.test` → Important                                | Important: sender beats domain                                                       | VIP             |
| 4    | `z`, which removes the sender pin                               | VIP                                                                                  | VIP             |
| 5    | `z`, which removes the domain pin                               | Team                                                                                 | Important       |
| 6    | label Ops → VIP                                                 | VIP: two label pins match, and VIP comes first in split order                        | Important       |
| 7    | reorder Team above VIP in the sheet                             | Team                                                                                 | Important       |
| 8    | sender `x@corp.test` → Important again, then turn Important off | Team: the sender pin's split is off, so the pin is skipped and the label pins decide | Other           |
| 9    | turn Important on                                               | Important: the pin was kept while its split was off                                  | Important       |

- **Also:**
  - No step makes a Gmail request.
  - Every `z` comes before any settings change, so this case does not depend on Q9.

#### SPL-04 · P1 · Pinning from the palette replaces an older pin; `z` restores it; a sender pin holds across syncs

_ST · §Split Inbox; §Undo ("A pin entry stores the pin it replaced") · Done when 3_

- **Seed:** `standard`.
- **Steps:**
  1. On `receipt`, run Command palette → "Move to Important".
  2. Choose the sender. Then run "Move to Other" and choose the sender. Then `z`, then `z`.
  3. Pin the sender `news@letters.test` to Important. `deliver` a new list message from `news@letters.test`. Then `poll()`, `expireHistory(A)`, `poll()`, `restart()`.
- **Expect:**
  - In step 1, the choices are exactly: sender `billing@shop.test`, domain `shop.test`, and label Receipts (the thread's user labels).
  - Moving to Important moves `receipt` there at once.
  - Moving to Other moves it back to Other: the new pin replaced the old one, so a sender is pinned to one split at a time.
  - The first `z` restores the replaced pin, and `receipt` goes back to Important.
  - The second `z` removes the pin, and `receipt` goes back to Other.
  - In step 3, the new message's thread lands in Important. It stays in Important after the poll, after the full resync, and after the restart.

#### SPL-05 · P2 · Your own reply does not move a thread

_EG · §Where a conversation lands ("Your own reply does not move a thread")_

- **Seed:** `standard`. Add a custom split "Mine" with the query `from:ana@acme.test`.
- **Steps:**
  1. Before replying, check that `own` (Kim's m1 and A's m2) is in Important.
  2. Reply to `addr`, send, `app.advance(10_000)`, `settle()`.
  3. `deliver` a thread whose only message is from `ana@acme.test` to herself.
- **Expect:**
  - `own` is in Important, because the rules read Kim's message.
  - After the reply, `addr` is still in Important, even though its latest message is from A.
  - The self-sent thread lands in Mine: when every message is from this account, the rules read the latest.

#### SPL-06 · P1 · Custom splits: add, order, rename, turn off, delete

_ST · §Split Inbox, §Custom rules_

- **Seed:** `standard`.
- **Steps and expect:**
  1. Open Command palette → "Split Inbox settings". The sheet is for A. It lists Important and Other. Other has no on/off switch and no delete. Important has a switch but no delete.
  2. Add "Receipts" with `label:receipts` (lower case) and save. `receipt` moves to a new first section, and the header reads `Receipts 1 · Important … · Other …`. Custom splits come first.
  3. Add "Venue" with `subject:venue OR subject:ticket`. `addr` and `replyto` move to it.
  4. Add "Carol" with `from:carol` above Venue. `addr` (from Carol, subject "venue") lands in **Carol**. Reorder Venue above Carol, and `addr` lands in **Venue**: the first match wins.
  5. Rename Venue to "Places". The header shows Places.
  6. Turn Places off. Its threads fall through to their next match. The sheet still shows Places with its query.
  7. Turn Places on. The threads move back.
  8. Pin the sender `billing@shop.test` to Receipts, then delete Receipts. `receipt` goes back to Other, and re-adding a split named Receipts does not bring the pin back.
  9. `label:CATEGORY_PROMOTIONS` as a query matches `promo`, which has that label on its own message.
- **Also:**
  - Saving makes no Gmail request.
  - Switching to B and opening its sheet shows only Important and Other.

#### SPL-07 · P2 · Starter splits, and a flat inbox

_DT · §Custom rules_

- **Seed:** `standard`.
- **Steps and expect:**
  1. The sheet offers Newsletters (`is:list`), Promotions (`label:CATEGORY_PROMOTIONS`), Social, and Updates. None is active until added or turned on.
  2. Add Newsletters. `news1`–`news3` move to it.
  3. Add Promotions. `promo` and `receipt` move to it.
  4. Turn every custom split off and Important off. The inbox is flat: every thread is in one section, Other, and the header shows only Other.

#### SPL-08 · P1 · Empty splits hide themselves, except after `g` `i` / `g` `o`

_DT · §Split Inbox, §Views · Done when 3_

- **Seed:** `standard`. Archive every Important thread (select them with `x` and `Shift+J`, then `e`).

| Important | Hide when empty  | Header and section                                                                                         |
| --------- | ---------------- | ---------------------------------------------------------------------------------------------------------- |
| empty     | on (the default) | The name is absent from the header, and there is no section.                                               |
| empty     | off              | The name shows in the header with no count. Whether an empty section header renders is not asserted (Q22). |
| not empty | on               | The name shows with its count.                                                                             |

- **Then:**
  - With Important empty and hidden, `g` `i` opens Inbox showing Important's header with no rows. `g` `o` highlights the first Other row.
  - Turn Important off. `g` `i` opens Inbox and does not show Important.

#### SPL-09 · P3 · Counts cap at `999+`

_BVA · §Split Inbox_

- **Seed:** `bulk1000`.
- **Expect:** the header shows `Important 999`. After one more delivery and `poll()`, it shows `Important 999+`.

#### SPL-10 · P2 · Clicking a split name scrolls to its section

_EP · §Window_

- **Seed:** `bulk`.
- **Steps:** Click "Other" in the list header.
- **Expect:** the Other section header is at the top of the list's viewport, and Other is now the active split name.

### 5.10 `snooze.spec.ts` (SNZ)

The clock is Monday 2026-11-02 10:00, America/New_York.

#### SNZ-01 · P1 · The snooze picker at a normal hour

_EP · §Snooze · Done when 4 (`h`)_

- **Steps:** Press `h` on `p1`. Check the options. Type "fri 9am". Press `Esc`.
- **Expect:**
  - The options are "Later today" (Mon 18:00), "Tomorrow morning" (Tue Nov 3, 08:00), and "Next week" (**Mon Nov 9**, 08:00: today is Monday, so it is the Monday after).
  - "fri 9am" offers Fri Nov 6, 09:00.
  - `Esc` closes only the picker. The row is still in Inbox.

#### SNZ-02 · P3 · "Later today" disappears at 18:00

_BVA · §Snooze (the arithmetic is unit-tested)_

- **Steps:** Advance to 17:59 and press `h`. Then advance to 18:00 and press `h`.
- **Expect:** at 17:59, "Later today" is offered. At 18:00 it is not.

#### SNZ-03 · P1 · Snoozing takes the thread out of Inbox, here and in Gmail

_ST · §Snooze · Done when 6_

- **Steps:** `hold(batchModify)`. Select `p1` and `p2` with `x`, press `h`, and choose "Tomorrow morning". Then `release`, `settle()`.
- **Expect:**
  - Both rows leave Inbox before the request is released.
  - `g` `h` lists both, each showing its wake time.
  - Neither is in Done.
  - In Gmail, both threads have no `INBOX` on their non-draft messages, and no Gmail label marks the snooze.
  - One `z` brings both back.

#### SNZ-04 · P1 · Snoozed threads wake on time, on launch, and after sleep, even offline

_DT, ST · §Snooze_

| Trigger         | Steps                                                                                         | Expect                                                                                                                                                                            |
| --------------- | --------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| timer           | Snooze `p1` until "Later today". `app.advance` to 18:00.                                      | `p1` is back in Inbox and gone from Snoozed. `gmail(A)` `p1` has `INBOX` again.                                                                                                   |
| offline at wake | `offline(true)`, then advance past 18:00                                                      | `p1` is in the local Inbox at once. Gmail still has no `INBOX` until `offline(false)` and `settle()`.                                                                             |
| launch          | Snooze `p1` until 18:00, then `restart()` with the clock moved to 19:00 while the app is down | While the app is down, `gmail(A)` `p1` has no `INBOX`. The first painted Inbox after launch already contains `p1`. Snoozed does not list it. Gmail gets `INBOX` after `settle()`. |
| Mac wake        | Snooze `p2` until 10:05. `wake()`, or jump the clock 10 min without firing timers.            | `p2` is woken.                                                                                                                                                                    |

#### SNZ-05 · P1 · `INBOX` added from outside clears a snooze; your own reply does not

_DT · §Snooze_

- **Seed:** `standard`. Before each row, snooze `p3` until "Next week". Then make the change and `poll()`.

| Change                                                                                | Snooze cleared?                                           | `p3` in Inbox? |
| ------------------------------------------------------------------------------------- | --------------------------------------------------------- | -------------- |
| A new message from Carol arrives in `p3` with `INBOX`                                 | yes                                                       | yes            |
| A message from `ben@bolt.test` (A's other account) arrives in `p3` with `INBOX`       | yes                                                       | yes            |
| Another client adds `INBOX` (`other(modify +INBOX)`)                                  | yes                                                       | yes            |
| Another client stars `p3`                                                             | no                                                        | no             |
| Reply sent from this app: open from Snoozed, `r`, send, `advance(10_000)`, `settle()` | no                                                        | no             |
| `e`, `#`, or `!` on the snoozed thread from Snoozed                                   | yes. It ends in Done, Trash, or Spam, and not in Snoozed. | no             |

#### SNZ-06 · P1 · The app's own label changes, echoed by history, do not clear a newer snooze

_EG · §Snooze ("Undoing a snooze and snoozing again before the next poll leaves the new snooze in place")_

- **Steps:**
  1. Snooze `p4` until Tue 08:00, then `settle()`.
  2. Press `z`; Gmail gets `+INBOX`.
  3. Before any poll, press `h` on `p4` again and choose Fri 09:00. `settle()`.
  4. `poll()`. The history batch contains both the app's `+INBOX` and its `−INBOX`.
- **Expect:**
  - `p4` is still in Snoozed until Fri 09:00, and not in Inbox.
  - Gmail has no `INBOX` on `p4`.

#### SNZ-07 · P1 · Undo, `Shift+E`, and `e` on snoozed threads

_ST · §Snooze, §Undo_

| Sequence                                                               | Expect                                                                                                                                         |
| ---------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------- |
| `h` `p1` (Tue 08:00), then `z`                                         | `p1` is in Inbox with no snooze. Gmail has `+INBOX`.                                                                                           |
| `h` `p1` (Tue 08:00), then from Snoozed `h` `p1` (Fri 09:00), then `z` | `p1` is snoozed until **Tue 08:00** again and is not in Inbox.                                                                                 |
| `h` `p1`, then `Shift+E` from Snoozed                                  | The snooze is cleared, `p1` is in Inbox, and Gmail has `+INBOX`.                                                                               |
| `h` `p1`, then `e` from Snoozed, then `z`                              | After `e`, `p1` is in Done, with no snooze and not in Inbox. After `z`, it is back in Snoozed with wake time Tue 08:00 and still not in Inbox. |

#### SNZ-08 · P2 · `Enter` before the typed time's options arrive snoozes until that time

_EG · §Snooze_

- **Steps:**
  1. On `p1`, press `h`. Park the page's `snooze_options` invokes for anything typed, type "fri 9am", press `Enter`.
  2. Release, `settle()`, open Snoozed.
- **Expect:** the picker closes and `p1` wakes Fri Nov 6, not at the first option shown before the reply (Later today).

### 5.11 `compose-reply.spec.ts` (REP)

#### REP-01 · P1 · `r` opens the reply composer under the thread, ready to type

_EP · §Reply composer · Done when 4, 8_

- **Seed:** `standard`. Open `plan`.
- **Steps:** Press `r`, then type "Thanks".
- **Expect:**
  - The composer appears below m12, the last message. The thread above it is still rendered: the subject heading and every message are in the DOM, and scrolling up shows them.
  - The composer is fully inside the viewport.
  - "Thanks" lands in the editor.
  - The selection bar is on m12, the target.
  - To is prefilled with Dave.
  - The hint bar shows `⌘Enter to Send · Esc to close · ⌘K for commands` (P3 detail).

#### REP-02 · P1 · What `r`, `Enter`, and `f` do on the list and in a conversation

_DT · §Reply composer, §Keyboard_

- **Seed:** `standard`. On the list, highlight `addr`. In a conversation, use `plan` with m9 selected (`p` three times).

| Key     | On the list                                                                  | In a conversation                                      |
| ------- | ---------------------------------------------------------------------------- | ------------------------------------------------------ |
| `r`     | Opens `addr` with a reply composer aimed at its latest message. To is Carol. | Reply aimed at m9. Its quote header names m9's sender. |
| `Enter` | Opens `addr` only. No composer.                                              | **Reply all** aimed at m9. Cc is filled.               |
| `f`     | Opens `addr` with a forward aimed at its latest message. To is empty.        | Forward aimed at m9.                                   |

#### REP-03 · P1 · Reply and reply-all addressing

_DT · §Addressing_

- **Seed:** `standard`. For each row, start a reply (`r`) or reply all (`Enter`) on the latest message. Check the composer's chips and From field. Then send one reply and one reply all per row, `advance(10_000)`, `settle()`, and check the sent message's `From`, `To`, and `Cc` headers in Gmail.

| Target message                                                           | Reply To            | Reply all To        | Reply all Cc                | From                                       |
| ------------------------------------------------------------------------ | ------------------- | ------------------- | --------------------------- | ------------------------------------------ |
| `addr`: From Carol; To ana@acme.test, dave; Cc erin                      | carol               | carol               | dave, erin                  | `ana@acme.test`                            |
| `replyto`: From Ivy, Reply-To tickets@help.test; To ana@acme.test; Cc jo | `tickets@help.test` | `tickets@help.test` | jo                          | `ana@acme.test`                            |
| `alias`: From Gus; To ana@ames.dev, hal                                  | gus                 | gus                 | hal (the alias is left out) | `ana@ames.dev`: the address it was sent to |
| `own`: latest From ana@acme.test; To kim; Cc lee                         | kim                 | kim                 | lee                         | `ana@acme.test`: that message's From       |
| `ownAlias`: latest From ana@ames.dev; To kim                             | kim                 | kim                 | –                           | `ana@ames.dev`                             |
| `news1`: sent to a list address, not to any `sendAs`                     | news@letters.test   | news@letters.test   | –                           | `ana@acme.test`, the default               |

- **Not asserted:** messages sent to two of the account's addresses (Q3).

#### REP-04 · P1 · A sent reply stays on the thread and carries Gmail's quote

_EP · §Addressing, §Send and undo_

- **Steps:** Reply to `addr`. Click `•••` in the composer. Send, `advance(10_000)`, `settle()`.
- **Expect, composer:** the quote is a collapsed read-only block under the editor. `•••` shows it, and it is not part of the editable document.
- **Expect, Gmail:** the sent message has:
  - `threadId` equal to `addr`'s
  - `In-Reply-To` set to Carol's `Message-ID`, and `References` containing it
  - HTML with `<div class="gmail_quote">`, beginning with an `On <date>, Carol … wrote:` line and Carol's text
  - an `X-Ubermensch-Draft` header
- **Expect, conversation:** the reply is the latest message, and no Sending row remains.

#### REP-05 · P1 · The reply follows the selected message

_ST · §Addressing ("After forwarding Alice's mail to Bob … `p` to Alice's message, then `r`, answers Alice")_

- **Steps:**
  1. Open `fwd`. Press `f`, add Kim as the recipient, send, `advance(10_000)`, `settle()`.
  2. Press `r`.
  3. Press `Esc` `Esc`, then `p`, then `r`.
- **Expect:**
  - In step 2, To is Kim: the forward is now the latest message.
  - In step 3, To is Alice, and the quote header names Alice.

#### REP-06 · P1 · Forward quotes the target, carries its attachments, and stays on the thread

_EP · §Addressing_

- **Steps:** In `fwd`, press `f`. Check the composer. Add Kim, send, `advance(10_000)`, `settle()`.
- **Expect, composer:**
  - To is empty, and From is the default `ana@acme.test`.
  - `contract.pdf` is listed as an attachment.
  - The quote block begins with `---------- Forwarded message ---------`, followed by Alice's From, Date, Subject, and To lines.
- **Expect, Gmail:** the sent message has:
  - the same `threadId`, with `In-Reply-To` and `References` set
  - `contract.pdf` with the original bytes
  - `logo.png` as an inline part with a Content-ID, referenced from the HTML as `cid:` and not as `data:`
- **Not asserted:** a forward subject prefix (Q5).

#### REP-07 · P1 · Which draft `r`, `Enter`, and `f` resume

_DT · §Reply composer_

- **Seed:** `standard`, thread `plan`. Build each row's drafts through the UI: start, type a marker, `Esc` `Esc`.

| Drafts already on the thread                 | Selection and key         | Result                                       |
| -------------------------------------------- | ------------------------- | -------------------------------------------- |
| reply R1 aimed at m12                        | m12, `r`                  | R1 is resumed, with its marker.              |
| R1                                           | m12, `Enter`              | R1 is resumed: `Enter` is a reply.           |
| R1                                           | m12, `f`                  | A **new** forward. R1 stays as a stub.       |
| R1, and forward F1 aimed at m12              | m12, `f`                  | F1 is resumed.                               |
| R2 aimed at m9                               | m12, `r`                  | A new reply aimed at m12. R2 is not resumed. |
| R2                                           | m9 (`p` three times), `r` | R2 is resumed.                               |
| R2                                           | click R2's stub           | R2 is resumed, and m9 becomes selected.      |
| R1 (older) and R3 (newer), both aimed at m12 | m12, `r`                  | R3, the newest, is resumed.                  |

- **Also:**
  - Every draft shows as a collapsed draft stub below the last message.
  - Whenever a composer opens, its target message is selected (§Reply composer: "Whenever a reply composer opens, by any path, its target message is selected").

#### REP-08 · P1 · A half-written reply survives new replies

_ST · §Reply composer_

- **Steps:**
  1. Open `p2`; its latest is m1. Press `r`, type "half", `Esc` `Esc`.
  2. `deliver` m2 from Pia into `p2`'s thread. `poll()`.
  3. Open `p2`; the selection is on m2. Press `r`.
- **Expect:**
  - The "half" draft is resumed: it was started on the latest message, which was m1 at the time.
  - Its target is m1, so m1 is selected.
  - Sending it replies to m1's sender.

#### REP-09 · P2 · Drafts from Gmail get a kind and a target

_DT · §Reply composer_

- **Seed:** `drafts`.

| Gmail draft              | Kind                                            | Aimed at                                    | Resumed by                                                                         |
| ------------------------ | ----------------------------------------------- | ------------------------------------------- | ---------------------------------------------------------------------------------- |
| `dNew`                   | new                                             | –                                           | `Enter` on its Drafts row opens the `c` screen                                     |
| `dFwd` ("Fwd: Contract") | forward                                         | Alice's message (from `In-Reply-To`)        | `f` in `fwd`, and not `r`                                                          |
| `dLatest`                | reply, counted as started on the latest message | p4's latest                                 | `r` on the latest; still `r` on the new latest after a new message arrives in `p4` |
| `dEarly`                 | reply                                           | plan's m3                                   | `r` only after selecting m3                                                        |
| `dUnknown`               | reply                                           | p6's latest (`In-Reply-To` matches nothing) | `r` on the latest                                                                  |

- **Also:** the kind sticks. Remove "Fwd:" from `dFwd`'s subject in Gmail (`updateDraft`) and `poll()`. `f`, not `r`, still resumes it.

#### REP-10 · P1 · Closing and sending the reply composer keep the conversation open

_ST · §Reply composer_

- **Steps:**
  1. In `p3`, press `r` and type.
  2. Press `Esc`, then `Esc` again. `settle()`.
  3. Press `r`, then `Cmd+Enter`.
  4. Press `n` and `p` while the send is pending.
  5. `advance(10_000)`, `settle()`.
- **Expect:**
  - The first `Esc` blurs the editor and leaves the composer on screen.
  - The second `Esc` closes it. The conversation stays open, a draft stub shows below the last message, and Gmail has the draft.
  - `r` resumes the draft.
  - After `Cmd+Enter`, the composer closes and the conversation stays open. A "Me … Sending" row shows at the bottom.
  - `n` and `p` never select the Sending row.
  - After the send, the Sending row is replaced by the real message, and the message count goes up by one.

#### REP-11 · P2 · A draft read from Gmail is split at its `gmail_quote`

_EP · §Addressing ("For a draft read from Gmail, everything from the `gmail_quote` element onward becomes that block")_

- **Seed:** `drafts`.
- **Steps:** Resume `dLatest`. Send, `advance(10_000)`, `settle()`.
- **Expect:**
  - The editor holds only "My text".
  - The quote block holds the rest.
  - The sent HTML has exactly one `gmail_quote`.

#### REP-12 · P2 · A Gmail draft's attachments are downloaded before its first upsert

_ST · §Addressing_

- **Seed:** `drafts`.
- **Steps:**
  1. Resume `dLatest`, type " more", `settle()`.
  2. Variant: `offline(true)`, resume, type, `offline(false)`, `settle()`.
- **Expect:**
  - In Gmail, the draft still carries `spec.pdf` with the same bytes, together with the new text.
  - In the request log, `attachments.get` comes before `drafts.update`.
  - In the offline variant, the composer opens while offline, and the result after reconnecting is the same.

#### REP-13 · P1 · Reply keys pressed before the conversation loads open one composer

_EG · §Keyboard ("a stray `r` leaves nothing behind")_

- **Steps:**
  1. Highlight `p3`. Park the page's submits, press `r` then `f`, release, `settle()`.
  2. `Esc` `Esc`, `settle()`.
  3. `Esc`, highlight `p5`. Park the submits, press `r`, `Esc`, `Enter`, release, `settle()`.
- **Expect:**
  - After step 1, one composer is open, a forward (To empty).
  - After step 2, the conversation has no draft stub and Gmail has no draft.
  - After step 3, `p5` is open with no composer: the reply was for a visit that ended.

#### REP-14 · P2 · Compose keys work while focus is inside a message

_EG · §Keyboard (compose blurred)_

- **Steps:** open `p3`, press `r`, type "Kept". Click inside the message body, then press `Esc` `Esc`, `settle()`.
- **Expect:** the composer closes, and the conversation shows the kept draft's stub.

### 5.12 `compose-new.spec.ts` (NEW)

#### NEW-01 · P1 · `c` replaces the screen, and closing it returns to where it came from

_DT · §Reply composer, §Keyboard · Done when 4_

| Started from             | After `Esc` `Esc` with typed content | After sending |
| ------------------------ | ------------------------------------ | ------------- |
| Inbox, row 3 highlighted | Inbox, row 3 highlighted             | the same      |
| `p1` open                | `p1` open                            | the same      |
| Starred                  | Starred                              | the same      |

- **Also:** the "New Message" screen replaces the list or conversation, To has focus, and the Subject field shows.

#### NEW-02 · P1 · Compose and send a formatted message, including the last keystrokes

_ST · §Compose, §Send and undo, §UI state ("Send … first flush the composer's pending save")_

- **Steps:**
  1. Press `c` and type "kim@kite.test,".
  2. `Cmd+Shift+S`, then type "Plan".
  3. `Cmd+Shift+M`, then type "Hello", bold "bold" (`Cmd+B` before and after), italic, underline, strike with `Cmd+Shift+X`, a `Shift+Enter`, a bulleted item (`Cmd+Shift+8`), a numbered item (`Cmd+Shift+7`), and a link with `Cmd+K` and "example.org" `Enter`.
  4. Type "end" and press `Cmd+Enter` at once.
  5. `advance(10_000)`, `settle()`.
- **Expect, UI:** the previous screen returns, and a "Sending…" toast shows with **Undo**.
- **Expect, Gmail:** the sent message has:
  - From `ana@acme.test`, To Kim, and Subject "Plan"
  - HTML with `<strong>`, `<em>`, `<u>`, `<s>`, `<br>`, `<ul><li>`, `<ol><li>`, and an `<a>` whose `href` contains "example.org"
  - a body ending with "end", with nothing lost to the save delay
- **Also:** Sent lists the message, and no Gmail draft is left.

#### NEW-03 · P2 · TipTap shortcuts that clash with the compose table are replaced

_DT · §Compose_

| Key in a focused editor | Expect                          | Must not happen |
| ----------------------- | ------------------------------- | --------------- |
| `Cmd+Shift+B`           | the Bcc field shows, with focus | a blockquote    |
| `Cmd+Shift+S`           | Subject gets focus              | strikethrough   |
| `Cmd+Enter`             | sends                           | a hard break    |
| `Shift+Enter`           | `<br>` in the body              | –               |
| `Cmd+Shift+X`           | strikethrough                   | –               |

#### NEW-04 · P2 · Compose field shortcuts work whether the editor is focused or blurred

_DT · §Compose_

- **Steps:** Run each row twice: once with the editor focused, once after one `Esc` (blurred).

| Key           | Expect                                                            |
| ------------- | ----------------------------------------------------------------- |
| `Cmd+Shift+O` | To has focus                                                      |
| `Cmd+Shift+C` | Cc shows and has focus                                            |
| `Cmd+Shift+B` | Bcc shows and has focus                                           |
| `Cmd+Shift+F` | the From picker opens, listing `ana@acme.test` and `ana@ames.dev` |
| `Cmd+Shift+S` | Subject has focus                                                 |
| `Cmd+Shift+M` | the editor has focus                                              |
| `Cmd+Shift+A` | the file chooser opens                                            |

#### NEW-05 · P1 · `Esc` in compose: the first blurs, the second closes; while blurred, only a few keys work

_ST · §Keyboard_

- **Steps:**
  1. Press `c`, fill To, Subject, and body.
  2. Press `Esc`. Then type `e`, `j`, `#`, `x`, `s`, `z`, `!`.
  3. Press `Enter` and type "more". Press `Esc`.
  4. Press `Cmd+K`, then `Esc`.
  5. Press `Esc`. `settle()`.
- **Expect:**
  - After the first `Esc`, the editor is blurred and the composer stays on screen.
  - The typed keys do nothing: the body is unchanged, the Inbox is unchanged, `req(batchModify)` is empty, and no selection appears.
  - `Enter` refocuses the editor, and "more" is appended.
  - `Cmd+K` opens a palette with exactly **Send**, **Attach a file**, and **Discard draft**. `Esc` closes it, and the composer stays, blurred.
  - The last `Esc` closes the composer. Drafts lists the draft, and Gmail has it with its subject and body.
- **Variant:** while blurred, `Cmd+Enter` sends.

#### NEW-06 · P1 · Shortcuts never fire while typing in compose

_EG · §Keyboard ("keystrokes go to that field, including keys held with Shift")_

- **Steps:** In a reply editor, type "Je suis #1! E? K/O x z". Type in the Subject and To fields in the same way.
- **Expect:**
  - The text appears exactly as typed.
  - Nothing is archived, trashed, spammed, or selected, and nothing is undone.
  - `req(batchModify)` is empty.

#### NEW-07 · P1 · A fresh composer closed untouched leaves nothing behind

_DT · §Keyboard ("A stray `r` leaves nothing behind")_

- **Seed:** `standard`. After each row, `settle()`. Check the draft stub, the Drafts view, and `gmail(A).drafts`.

| Opened with                    | Change before closing             | Closed by   | Result                              |
| ------------------------------ | --------------------------------- | ----------- | ----------------------------------- |
| `r`                            | none                              | `Esc` `Esc` | nothing left, locally or in Gmail   |
| `Enter` (reply all)            | none                              | `Esc` `Esc` | nothing left                        |
| `f` (with default attachments) | none                              | `Esc` `Esc` | nothing left                        |
| `c`                            | none                              | `Esc` `Esc` | nothing left                        |
| `r`                            | none                              | `Ctrl+2`    | nothing left                        |
| `r`                            | typed in the body                 | `Esc` `Esc` | kept: stub, Drafts row, Gmail draft |
| `r`                            | added a To chip                   | `Esc` `Esc` | kept                                |
| `r`                            | From changed to `ana@ames.dev`    | `Esc` `Esc` | kept                                |
| `f`                            | removed the forwarded attachment  | `Esc` `Esc` | kept                                |
| `c`                            | subject only                      | `Esc` `Esc` | kept                                |
| a resumed draft                | none                              | `Esc` `Esc` | kept                                |
| a draft reopened by undo send  | none                              | `Esc` `Esc` | kept (DEVIATIONS)                   |
| `r`                            | typed, then deleted back to empty | `Esc` `Esc` | **not asserted** (Q4)               |

#### NEW-08 · P2 · `Cmd+K` in compose

_DT · §Keyboard, §Compose_

| Editor state                  | `Cmd+K`                                                                                                                      |
| ----------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| focused, with "here" selected | Opens the link input. Typing "example.org" and `Enter` links "here" to a URL containing "example.org".                       |
| blurred                       | Opens the draft-actions palette. Choosing **Discard draft** closes the composer, and the draft is gone locally and in Gmail. |

#### NEW-09 · P1 · Sending with no recipients never loses the draft; text still in the To field is sent

_DT · §Recipients_

| To field at `Cmd+Enter`               | Result                                                                                                                                                                                                                            |
| ------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| empty, no chips                       | The message is not sent, and the draft is not lost. Either Send refuses before the outbox, which the implementation does with a toast, or Gmail rejects it and the draft comes back as in SND-06. The SPEC does not choose (Q26). |
| "pat@p.test" typed but not yet a chip | Sent: the message's To includes `pat@p.test`.                                                                                                                                                                                     |

#### NEW-10 · P2 · From picker and the sending account

_EP · §Addressing, §Switching_

- **Steps:**
  1. On A, press `c`. Check From. Press `Cmd+Shift+F`, choose `ana@ames.dev`, then send.
  2. On B, press `c` and send.
- **Expect:**
  - On A, From starts as `ana@acme.test`. The message is sent From `ana@ames.dev`.
  - On B, there is no From row (B has one `sendAs`). The message is in `gmail(B)` and not in `gmail(A)`.

#### NEW-11 · P1 · `Cmd+Shift+,` discards locally and in Gmail

_ST · §Compose, §Drafts_

- **Steps:**
  1. Press `c`, type, `settle()`. The Gmail draft exists.
  2. Press `Cmd+Shift+,`, then `settle()`.
  3. Repeat with the resumed Gmail draft `dNew` (seed `drafts`).
- **Expect:**
  - The composer closes, and a "Draft discarded" toast shows.
  - Drafts has no row for it.
  - `drafts.delete` was called, and `gmail(A).drafts` no longer has the draft.
  - The resumed Gmail draft behaves the same way.

#### NEW-12 · P2 · Opening another composer first closes the current one, as a second `Esc` would

_DT · §UI state (Actions: "Anything that takes an open composer off screen …")_

| Current composer | What takes it off screen           | Its draft afterwards |
| ---------------- | ---------------------------------- | -------------------- |
| reply, touched   | `c`                                | kept                 |
| reply, untouched | `c`                                | discarded            |
| `c`, touched     | clicking the Inbox rail icon       | kept                 |
| reply, touched   | `Enter` on another draft in Drafts | kept                 |

### 5.13 `recipients.spec.ts` (RCP)

#### RCP-01 · P1 · Autocomplete: at most six suggestions, keyboard choice, `Esc` stays in the field

_ST, BVA · §Recipients · Done when 8_

- **Seed:** `contacts`.
- **Steps:**
  1. Press `c` and type "mar".
  2. Press `↓`, `↓`, `↑`, then `Enter`.
  3. Type "mar" again and press `Tab`.
  4. Type "mar" and press `Esc`, then type "k".
- **Expect:**
  - In step 1, exactly 6 suggestions show (there are 8 matching contacts). Each has a name and an address, and the matching part is bold.
  - In step 2, the second suggestion is highlighted, and `Enter` turns it into a chip.
  - In step 3, `Tab` accepts the highlighted suggestion.
  - In step 4, `Esc` closes the list, and To keeps focus: the next typed "k" lands in To.

#### RCP-02 · P1 · Ranking and exclusions come from synced mail

_DT · §Recipients (the scoring math is unit-tested)_

- **Seed:** `contacts`. `settle()` so the backfill finishes.
- **Steps:** In To, type "s".
- **Expect:**
  - The order is `sam@x.test` and `sara@x.test` (the sent tier), then `sue@recv.test` (received only), then `sid@cc.test` (co-recipient only).
  - `sly@list.test`, `noreply@svc.test`, `no-reply@svc.test`, `spammy@spam.test`, `drafty@d.test`, and `ana@ames.dev` never appear.
  - `sara@x.test` is present only because of the contacts backfill: her only sent mail is 200 days old.
  - Typing "recv" suggests `sue@recv.test`, matched by the start of its domain. Typing "ue" does not suggest it: a match never starts in the middle of the local part. One check of each is enough; matching itself is unit-tested.

#### RCP-03 · P2 · Ways to turn typed text into a chip

_DT · §Recipients_

- **Seed:** `standard`. Use addresses that have no contacts, so no suggestion list opens.

| Typed           | Then                    | Result                                    |
| --------------- | ----------------------- | ----------------------------------------- |
| `kim@kite.test` | `,`                     | chip                                      |
| `kim@kite.test` | space                   | chip                                      |
| `kim`           | space                   | **no** chip: the text stays               |
| `lee@kite.test` | `Tab`                   | chip                                      |
| `lee@kite.test` | `Enter`                 | chip                                      |
| `zed@z.test`    | click outside the field | chip                                      |
| `pat@p.test`    | `Cmd+Enter`             | sent with `pat@p.test` in To (see NEW-09) |

#### RCP-04 · P2 · `Backspace` and paste

_ST · §Recipients_

- **Steps:**
  1. With two chips and an empty field, press `Backspace`, then `Backspace` again.
  2. Paste `a@x.test, Bob <b@y.test>; c@z.test\nd@w.test`.
- **Expect:**
  - The first `Backspace` selects the last chip. The second removes it.
  - The paste makes 4 chips, and Bob's chip shows "Bob".

#### RCP-05 · P1 · An invalid chip blocks send until it is fixed

_ST · §Recipients_

- **Steps:**
  1. Type `kim@kite,`.
  2. Press `Cmd+Enter`.
  3. Remove the chip with `Backspace` `Backspace`, add `kim@kite.test`, and send.
- **Expect:**
  - The chip shows in `danger`.
  - Send is refused with a toast naming the address. No outbox row, and no Sending toast.
  - After the fix, the message sends.

#### RCP-06 · P2 · `Tab` moves between fields unless the suggestion list is open

_DT · §Keyboard_

- **Steps:** In a new message, press `Tab` and `Shift+Tab` through the fields. Then type "mar" in To and press `Tab`.
- **Expect:**
  - `Tab` goes To → Subject → body, and through Cc when it is shown. `Shift+Tab` goes back.
  - With the list open, `Tab` accepts the suggestion instead of moving.

#### RCP-07 · P2 · Cc and Bcc stay hidden until used

_EP · §Recipients_

- **Expect:**
  - A new message has no Cc or Bcc row. `Cmd+Shift+C` shows Cc.
  - Reply all on `addr` shows Cc, already filled.
  - A Bcc recipient added with `Cmd+Shift+B` receives the message, and the To and Cc headers of the sent message do not list them.

#### RCP-08 · P2 · New mail updates the suggestions

_ST · §Recipients_

- **Steps:**
  1. `deliver` a message from "Quinn Q <quinn@q.test>" and `poll()`. Type "qui" in To.
  2. Send to Quinn. Type "qui" again.
- **Expect:**
  - In step 1, Quinn is suggested.
  - In step 2, Quinn is still suggested, now ranked above contacts that are received-only, such as `quin@people.test`.

#### RCP-09 · P2 · Suggestions that answer after a chip is made stay closed

_EG · §Recipients_

- **Seed:** `contacts`.
- **Steps:**
  1. Press `c`. Pause page timers and park the page's `autocomplete` invokes. Type "sam,".
  2. Release, and wait until the page has no invoke pending.
  3. Press `Tab`, resume timers, `settle()`.
- **Expect:**
  - After step 2, no suggestion list is open.
  - After step 3, To holds only the "sam" chip.

#### RCP-10 · P2 · The autosave leaves an address being typed alone

_EG · §Recipients, §Drafts (autosave)_

- **Steps:**
  1. Press `c`, type "kim@kite.test,", then "lee@ki".
  2. Let the autosave the chip scheduled fire (page clock `runFor(500)`), `settle()`.
- **Expect:** To still holds only Kim's chip, and its input still reads "lee@ki" with focus. Only send and close turn typed text into a chip.

### 5.14 `attachments.spec.ts` (ATT)

#### ATT-01 · P1 · Attach files and send them

_ST · §Drafts, §Compose_

- **Steps:**
  1. Press `c` and `Cmd+Shift+A`. Choose `report.pdf` (100 KB) and `photo.png` in the file chooser.
  2. Attach one more file with the paperclip button.
  3. Remove `photo.png` with its ×.
  4. `settle()`, then send, `advance(10_000)`, `settle()`.
- **Expect:**
  - Chips show each file name.
  - After the first `settle()`, the Gmail draft carries the attachments.
  - The sent message carries `report.pdf` with the same bytes and the `application/pdf` type, plus the paperclip file. It does not carry `photo.png`.

#### ATT-02 · P1 · Received attachments download only when opened

_ST · §Reading_

- **Seed:** `standard`, after the connect sync has settled.
- **Steps:**
  1. Check the request log.
  2. Open `fwd`. Click `contract.pdf`.
  3. Click it again.
- **Expect:**
  - In step 1, the log has no `attachments.get`.
  - The chip shows "contract.pdf".
  - The first click makes exactly one `attachments.get`. `openedFiles` gets a path whose bytes equal the original.
  - The second click makes no new request, and the file is opened again.

#### ATT-03 · P1 · A forward made offline still sends its attachments

_ST · §Addressing ("a forward works offline"), §Send and undo step 1_

- **Steps:**
  1. `offline(true)`. In `fwd`, press `f`, add Kim, and send.
  2. `advance(10_000)`.
  3. `offline(false)`, then `settle()`.
- **Expect:**
  - After step 2, the toast says "Email delayed".
  - After reconnecting, the sent forward carries `contract.pdf` with the original bytes.
  - In the log, `attachments.get` comes before `drafts.create` or `drafts.update`, and before `drafts.send`.

#### ATT-04 · P2 · MIME tree shape and encoded headers

_DT · §Drafts_

| Message                           | Top part            | Structure                                                                                                                                  |
| --------------------------------- | ------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ |
| a new message with no attachments | `multipart/related` | [`multipart/alternative`]                                                                                                                  |
| a new message with one attachment | `multipart/mixed`   | [`multipart/related` [`multipart/alternative`], the attachment]                                                                            |
| a forward of `fwd`                | `multipart/mixed`   | [`multipart/related` [`multipart/alternative`, `image/png` with a Content-ID], `application/pdf`]. The HTML refers to the image by `cid:`. |

- **Also:** a subject of "Grüße – 日本" arrives decoded and identical in Gmail.

#### ATT-05 · P1 · A send while a file is still attaching carries the file

_EG · §Compose, DEVIATIONS (a file still attaching)_

- **Steps:**
  1. Press `c`, add Kim, a subject, and a body.
  2. Park the page's `add_attachment` invoke. Choose `report.pdf`, then press `Cmd+Enter`.
  3. Release, `settle()`, `advance(10_000)`, `settle()`.
- **Expect:** the sent message carries `report.pdf` with the original bytes.

#### ATT-06 · P1 · Closing a fresh composer while a file is attaching keeps the draft

_EG · §Keyboard (untouched rule), DEVIATIONS (a file still attaching)_

- **Steps:**
  1. Press `c`. Park the page's `add_attachment` invoke and choose `notes.txt`.
  2. Press `Esc` `Esc`.
  3. Release, `settle()`.
- **Expect:**
  - After step 2, the composer is gone.
  - Gmail has one draft, carrying `notes.txt`. The draft was not discarded as untouched.

### 5.15 `drafts.spec.ts` (DRF)

#### DRF-01 · P1 · Drafts autosave and update a single Gmail draft

_ST · §Drafts_

- **Steps:**
  1. In `p3`, press `r`, type "one", `settle()`.
  2. Type " two", `settle()`.
  3. Press `Esc` `Esc`, then `g` `d`.
- **Expect:**
  - After step 1, Gmail has one draft in `p3`'s thread with body "one" and `X-Ubermensch-Draft` set to the local id.
  - After step 2, the same Gmail draft id has body "one two" (a `drafts.update`), and there is still one draft.
  - Drafts lists it once.

#### DRF-02 · P1 · `Enter` in Drafts opens a draft according to its kind

_DT · §Reading ("In Drafts, `Enter` on a new-message draft …")_

- **Seed:** `drafts`.

| Drafts row        | `Enter`                                                                         |
| ----------------- | ------------------------------------------------------------------------------- |
| `dNew`            | Opens the `c` screen with its content. `Esc` `Esc` returns to Drafts.           |
| `dLatest` (reply) | Opens `p4` with the draft in the inline reply composer. Its target is selected. |
| `dFwd` (forward)  | Opens `fwd` with the forward composer.                                          |

#### DRF-03 · P1 · Edits made to a draft in Gmail, with the draft closed or open

_DT · §Drafts_

- **Seed:** `drafts`, draft `dLatest`. In each row, edit the draft in Gmail (`updateDraft` with "from web"), then `poll()`.

| When the Gmail edit happens | Edits in the composer | Expect                                                                                                          |
| --------------------------- | --------------------- | --------------------------------------------------------------------------------------------------------------- |
| the draft is closed         | –                     | Resuming shows "from web".                                                                                      |
| the draft is open           | none                  | The editor is **not** overwritten while open. After `Esc` `Esc`, `settle()`, and resuming, it shows "from web". |
| the draft is open           | the user types "mine" | The editor keeps "mine". After closing and `settle()`, the Gmail draft holds the app's text. (Q24)              |

#### DRF-04 · P1 · A draft deleted or sent elsewhere

_DT · §Drafts_

- **Seed:** `drafts`. Run each row twice: once with `deleteDraft`, once with `sendDraft` (sent from a phone). `poll()` after the change.

| Composer | Expect                                                                                                                                                                                                                       |
| -------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| closed   | The stub and the Drafts row disappear.                                                                                                                                                                                       |
| open     | The composer stays with its text, and a toast says "Draft was sent or deleted elsewhere". The next keystroke and `settle()` call `drafts.create`, which gives a new Gmail draft id. Gmail then has exactly one draft for it. |

#### DRF-05 · P1 · Discarding while offline is queued and replayed in order

_ST · §Drafts, §Offline_

- **Steps:**
  1. `offline(true)`. Resume `dLatest`, type, and press `Cmd+Shift+,`.
  2. `offline(false)`, then `settle()`.
- **Expect:**
  - The draft disappears locally at once.
  - After reconnecting, the Gmail draft is deleted.
  - No queued upsert brings it back: `gmail(A).drafts` does not contain it.

#### DRF-06 · P1 · A draft written offline reaches Gmail exactly once

_ST · §Drafts, §Offline_

- **Steps:**
  1. `offline(true)`. Press `c`, fill To, Subject, and body, then `Esc` `Esc`.
  2. Open it again and change the body.
  3. `offline(false)`, then `settle()`.
- **Expect:**
  - After step 1, Drafts lists it as a local draft.
  - After reconnecting, Gmail has exactly one draft, with the final content.

#### DRF-07 · P2 · Sending cancels a queued upsert

_EG · §Drafts ("Send cancels any queued upsert for that draft")_

- **Steps:**
  1. `offline(true)`. Resume a draft that is already in Gmail, type "final", and send.
  2. `offline(false)`, `advance(10_000)`, `settle()`.
- **Expect:**
  - The sent body is "final".
  - No `drafts.update` after `drafts.send` appears in the log.
  - No Gmail draft is left.

#### DRF-08 · P2 · A webview reload or a crash with a composer open

_DT · §UI state (Subscriptions: "On load … an open composer … is closed as a second `Esc` would"; Actions: "On launch, Rust applies the untouched rule…")_

| Composer                       | Event                    | Result                     |
| ------------------------------ | ------------------------ | -------------------------- |
| `r`, touched                   | `page.reload()`          | Kept: stub and Gmail draft |
| `r`, untouched                 | `page.reload()`          | Discarded                  |
| `c`, touched (started fresh)   | `crash()`, then relaunch | Kept                       |
| `c`, untouched (started fresh) | `crash()`, then relaunch | Discarded on launch        |

- **Also:** after the reload, `e` on `p1` still reaches Gmail. Action numbers continue (see UIC-02).

#### DRF-09 · P3 · The app's own drafts are read back after the account is re-added

_EG · §Addressing, §Gmail errors (`X-Ubermensch-Draft` on sync)_

- **Steps:**
  1. Reply to `addr`, type "keep me", `settle()`.
  2. Remove A, then add A again. `settle()`.
- **Expect:**
  - Drafts lists exactly one draft for `addr`, reading "keep me".
  - Resuming it shows the quote as a separate block.
  - Gmail has one draft.

#### DRF-10 · P1 · Text deleted while its save is on the way stays deleted

_EG · §Actions (the mirror applies Rust's echo of a save)_

- **Steps:**
  1. Reply to `p3` and type "Hello". Park the page's submits and let the autosave fire (page clock `runFor(500)`).
  2. Select all and delete. Release, and wait until the save's echo (body "Hello") is in the mirror.
  3. Type "Bye", let the autosave fire, `settle()`.
- **Expect:**
  - After step 2, the editor stays empty.
  - After step 3, the editor reads "Bye", and the Gmail draft has "Bye" and no "Hello".

#### DRF-11 · P1 · Typing before a reply's first load keeps its filled fields

_EG · §Actions ("the body is editable at once, and later saves carry the full compose state")_

- **Steps:**
  1. Open `addr`. Park the page's submit that carries the composer's subscribe, press `r`, type "Early words", and let the autosave fire (page clock `runFor(500)`).
  2. Release, let the autosave fire, `settle()`.
  3. `Esc` `Esc`, `settle()`. Park the subscribe again, press `r` (it resumes the draft), type "More words", `Esc` `Esc`. Release, `settle()`.
- **Expect:**
  - After step 2, To shows Carol, the editor reads "Early words", and the Gmail draft has To Carol, Subject "Re: Offsite venue", and "Early words".
  - After step 3, the same Gmail draft still has To Carol and reads "Early words" then "More words".

### 5.16 `undo-send.spec.ts` (SND)

Outbox state machine (§Send and undo):

```
waiting --send_after passes--> sending --drafts.send accepted--> sent --toast closes--> (entry leaves history)
waiting --Undo--> cancelled: composer reopens
waiting --Gmail unreachable at send_after--> waiting, shown as "Email delayed"
sending --Gmail unreachable before drafts.send--> waiting
sending --no clear answer--> sending --drafts.get: 404--> sent | draft exists--> drafts.send again
sending --Undo--> held --resolves sent--> sent message trashed | resolves not sent--> notice, composer reopens
sending --rejected--> toast, composer reopens
sent --Undo, toast still open--> only that sent message is trashed
```

#### SND-01 · P1 · The send waits ten seconds, then goes out once

_ST, BVA · §Send and undo · Done when 9_

- **Steps:**
  1. Press `c`, address Kim, and send at t0.
  2. `advance(9_999)`, `settle()`.
  3. `advance(1)`, `settle()`.
  4. `advance(5_000)`. Then press `z`.
- **Expect:**
  - After step 1, the toast says "Sending…" with **Undo**.
  - After step 2, `req(drafts.send)` is empty.
  - After step 3, `drafts.send` was called exactly once. The toast says "Sent". Sent lists the message, and Gmail has it.
  - After step 4, the toast has closed. `z` does **not** trash the sent message: there is no `TRASH` on it in Gmail.

#### SND-02 · P1 · Undo while waiting cancels the send and reopens the composer

_DT · §Send and undo · Done when 9_

| Undo with                          | Draft                                                                                 | Expect                                                  |
| ---------------------------------- | ------------------------------------------------------------------------------------- | ------------------------------------------------------- |
| the toast's **Undo**               | a reply in `p3`. Archive `p3` after sending, so the thread has left the current view. | `p3` reopens with the inline composer holding the text. |
| `z` (the send is the newest entry) | a new message                                                                         | The `c` screen reopens with the content.                |

- **Then, for both rows:**
  - `advance(60_000)`, `settle()`: `req(drafts.send)` is empty.
  - `Esc` `Esc` keeps the reopened draft: it stays in Drafts.
  - Sending again works.

#### SND-03 · P1 · `z` reaches a send only while it is the newest entry; the toast's Undo always does

_DT · §Undo, §Send and undo_

- **Steps:** Send S, a reply in `p3`. Then press `e` on `p4`.

| Next action                           | Result                                                                                    |
| ------------------------------------- | ----------------------------------------------------------------------------------------- |
| `z`                                   | Undoes the `e`: `p4` is back. S stays waiting and is sent at 10 s.                        |
| (fresh run) the toast's **Undo** on S | S is cancelled and its composer reopens. `p4` stays archived. A later `z` undoes the `e`. |

#### SND-04 · P1 · Undo after the send was accepted trashes only that message

_ST · §Send and undo ("Sent: trash only that sent message")_

- **Steps:**
  1. Reply to `addr` and send. `advance(10_000)`, `settle()`. The toast says Sent and is still open.
  2. Click **Undo**, then `settle()`.
- **Expect:**
  - In Gmail, only the sent reply has `TRASH`. Carol's message keeps `INBOX`.
  - `addr` stays in Inbox.
  - The conversation no longer shows the reply.

#### SND-05 · P1 · Undo while sending is held until the send resolves

_DT · §Send and undo_

- **Steps:**
  1. `hold(drafts.send)`. Send a reply in `p3`. `advance(10_000)`; the row is sending.
  2. Click **Undo**.
  3. Press `z`.
  4. Release with each outcome below.
- **Expect, before release:**
  - The toast no longer offers Undo. The implementation labels it "Undoing…"; that wording is not in the SPEC.
  - `z` undoes the previous entry instead, because the send's entry has already left the history.

| Release outcome                                      | Expect                                                                        |
| ---------------------------------------------------- | ----------------------------------------------------------------------------- |
| 200 (sent)                                           | The app trashes the sent message: Gmail has `TRASH` on it. No composer opens. |
| 400 (rejected, not sent)                             | A notice. The draft is back in a composer, in `p3`'s thread.                  |
| `drop` (not sent), then `drafts.get` finds the draft | Not asserted (Q6).                                                            |

#### SND-06 · P1 · A rejected send brings the draft back

_DT · §Send and undo, §Gmail errors_

- **Steps:** `fault(drafts.send, status 400 "Invalid To header")`. Send, then `advance(10_000)`.

| User state when the rejection arrives                 | Expect                                                                                                                                                                 |
| ----------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| B on screen, on its list                              | The app switches to A and reopens `p3` with the reply composer. An error toast shows Gmail's message.                                                                  |
| typing in another composer (`c` on A, editor focused) | No takeover. A toast offers **Open draft**. Clicking it closes the current composer as a second `Esc` would (kept, since it was touched) and opens the rejected draft. |
| idle, and the draft is a new message                  | The `c` screen opens with the draft.                                                                                                                                   |

#### SND-07 · P1 · Offline at send time: "Email delayed", then sent once online

_ST · §Send and undo, §Offline · Done when 9, 10_

- **Steps:**
  1. Reply to `p3` and send. `offline(true)`, then `advance(10_000)`.
  2. `offline(false)`, then `settle()`.
  3. Variant: before reconnecting, click **Undo**.
  4. Variant: `fault(drafts.create, drop)` at the build step.
- **Expect:**
  - After step 1, the toast and the pending row in the thread both say "Email delayed".
  - After step 2, the message is sent and the toast says "Sent".
  - In variant 3, the composer reopens and nothing is ever sent ("after the timer has passed while Gmail is unreachable").
  - In variant 4, the row goes back to waiting and retries. Exactly one message is sent.

#### SND-08 · P1 · `drafts.send` with no clear answer never sends twice

_DT · §Send and undo, §Gmail errors_

| Fault on the first `drafts.send` | What `drafts.get` finds | Expect                                                                                                                                           |
| -------------------------------- | ----------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| `noResponse` (Gmail sent it)     | 404                     | The row becomes sent. Its message id is the newest `SENT` message from A in the thread, and the conversation shows it. Exactly one sent message. |
| `drop` (Gmail did not send it)   | the draft               | `drafts.send` is called again. Exactly one sent message.                                                                                         |
| `statusAfterApply(500)`          | 404                     | Sent, and exactly one message.                                                                                                                   |
| `status(503)` (not applied)      | the draft               | Sent again, and exactly one message.                                                                                                             |

#### SND-09 · P1 · Outbox rows across a quit and relaunch

_DT · §Send and undo ("`Cmd+Q` does not send waiting rows early"), §Undo_

| State at quit                                    | Steps                                                                                                    | Expect                                                                                                                                                    |
| ------------------------------------------------ | -------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------- |
| waiting, 3 s into the window                     | `restart()` with the clock moved 1 min ahead while the app is down. `hold(drafts.send)` before relaunch. | Nothing is sent while the app is down. After launch, with the send held, the toast says "Email delayed". After release, the message is sent exactly once. |
| waiting, offline at relaunch                     | `restart()`                                                                                              | After launch, **Undo** reopens the composer, and nothing is sent.                                                                                         |
| sending, no answer (`noResponse`, Gmail sent it) | `crash()`, then relaunch                                                                                 | On launch, `drafts.get` returns 404. The row is sent, and there is exactly one message.                                                                   |
| sent, toast still open                           | `restart()`, then `z`                                                                                    | The send entry was dropped on launch. `z` does not trash the delivered mail.                                                                              |

#### SND-10 · P2 · Send toasts stay across accounts

_EP · §Switching ("Send toasts stay on screen across switches")_

- **Steps:** On A, send. Press `Ctrl+2`. Click **Undo** on the toast.
- **Expect:**
  - The toast is still visible on B.
  - Undo switches to A and reopens the composer.

#### SND-11 · P2 · Undoing a waiting send while another composer is open

_EG · §Send and undo ("If another composer is open, it is first closed as a second `Esc` would close it")_

| Other composer | Toast **Undo** on send S                                 |
| -------------- | -------------------------------------------------------- |
| `c`, touched   | The `c` draft is kept in Drafts, and S's composer opens. |
| `c`, untouched | The `c` draft is discarded, and S's composer opens.      |

#### SND-12 · P2 · The sent message shows without waiting for a poll

_EP · §Send and undo ("Rust then fetches that message and stores it at once")_

- **Steps:** Reply to `p3` and send. `advance(10_000)`, `settle()`. Do not call `poll()`.
- **Expect:**
  - The conversation shows the sent message, and the Sending row is gone.
  - In the log, a message or thread fetch for the sent id comes right after `drafts.send`.

### 5.17 `offline.spec.ts` (OFF)

#### OFF-01 · P1 · Airplane mode: read, search, autocomplete, and act; it all lands when the network returns

_ST · §Offline · Done when 10_

- **Seed:** `standard`, settled.
- **Steps:**
  1. `offline(true)`.
  2. Open `plan`.
  3. Press `/` and type "venue".
  4. Press `c` and type "car" in To.
  5. Press `e` on `p1`.
  6. Reply to `p2`, send, `advance(10_000)`.
  7. `offline(false)`, then `settle()`.
- **Expect while offline:**
  - `plan` opens with its bodies.
  - The search shows `addr` and "Results from this Mac".
  - Carol is suggested.
  - `p1` leaves Inbox with no toast.
  - The send toast says "Email delayed".
- **Expect after reconnecting:**
  - `gmail(A)` `p1` is archived.
  - The reply is in `p2`'s thread in Gmail, and the toast says "Sent".

#### OFF-02 · P1 · Every kind of queued action replays in order

_ST · §Offline (the list of queued actions)_

- **Steps:**
  1. `offline(true)`.
  2. In this order: `e` `p1`; `#` `p2`; `!` `p3`; `s` `p4`; `u` `p6`; `Shift+E` on `archivedOld` (found by search); `l` Travel on `fold`; create label "NewL" on `alias`; `h` on `own`; `Shift+E` on `ownAlias`, which was snoozed while online; `r` on `replyto` with typing (a draft upsert); discard of a draft already in Gmail.
  3. `offline(false)`, then `settle()`.
- **Expect:**
  - Gmail reflects each action.
  - The mutating requests in the log follow the action order, with `labels.create` before the `batchModify` that uses it.
  - No toasts.

#### OFF-03 · P1 · The queue survives a restart

_ST · §Offline ("The queue is SQLite … A restart does not drop it")_

- **Steps:** `offline(true)`. `e` on `p1`, `s` on `p2`. `restart()`, still offline. Then `offline(false)` and `settle()`.
- **Expect:**
  - After the restart, `p1` is still out of Inbox and `p2` is still starred.
  - After reconnecting, both changes are in Gmail.

#### OFF-04 · P1 · A queued action wins on the messages it recorded, without hiding newer mail

_EG · §Offline, §Archive, trash, star, read_

- **Steps:**
  1. `offline(true)`. `e` on `p1` (m1).
  2. While still offline, `other(modify m1 +STARRED +INBOX)`, and `deliver` m2 into `p1` with `INBOX` and `UNREAD`, with `poll: false`.
  3. `offline(false)`, then `settle()`.
- **Expect, Gmail:**
  - m1 has no `INBOX`: the queued action won.
  - m1 is still `STARRED`.
  - m2 still has `INBOX`.
- **Expect, local:** after the history sync, `p1` is in Inbox (because of m2), starred, and unread, which matches Gmail.

#### OFF-07 · P1 · A Gmail answer read before an action landed does not undo it

_EG · §Offline, §Sync_

- **Steps:**
  1. `holdAnswer("history.list")`, then `modify m1 +STARRED` from another client with `poll: true`. The history is read now (m1 with `INBOX`) and its answer parked.
  2. `e` on `p1`, then `settle()`: the archive lands in Gmail and leaves the queue.
  3. Release the history answer, then `settle()`.
- **Expect:**
  - Gmail's m1 had no `INBOX` before the release.
  - `p1` stays out of Inbox, and is in Done, starred.
- **Why:** the stale answer arrives after the action left the queue, so re-applying only queued actions lets it undo the archive. This was the snooze-at-launch bug: a wake's `+INBOX` undone by the first poll.

#### OFF-05 · P1 · A rejection during replay rolls back that one action, and replay continues

_ST · §Offline, §Gmail errors_

- **Steps:** `offline(true)`. `e` on `p1`, `s` on `p2`. `fault(batchModify, status 400 "Invalid label", {nth: 1})`. `offline(false)`, then `settle()`.
- **Expect:**
  - `p1` is back in Inbox, and an error toast shows the message.
  - `p2` is starred in Gmail.
  - The queue is empty: Settings' remove confirmation shows no queued actions.

#### OFF-06 · P2 · Losing the network during replay leaves the rest queued

_EG · §Offline ("Loss of network leaves the queued action in place")_

- **Steps:**
  1. `offline(true)`. `e` on `p1`, `p2`, and `p3`.
  2. `offline(false)` with `fault(batchModify, drop, {nth: 2})`.
  3. `settle()`. Clear the fault, then `settle()`.
- **Expect:**
  - `p2` and `p3` never reappear locally.
  - At the end, all three are archived in Gmail.
  - Each thread's change appears exactly once in the log, apart from the dropped attempt.

### 5.18 `gmail-errors.spec.ts` (ERR)

Outcomes by kind of call (§Gmail errors). "Silent" means no toast.

| Outcome                                          | Mail action (`batchModify`)                                                         | `drafts.create`              | `drafts.send`                            | sync, search, body fetch                                                      |
| ------------------------------------------------ | ----------------------------------------------------------------------------------- | ---------------------------- | ---------------------------------------- | ----------------------------------------------------------------------------- |
| no connection                                    | stays queued, silent (OFF-*)                                                        | queued                       | the row goes back to waiting (SND-07)    | offline, cached data still works                                              |
| sent, no response                                | retried, silent (ERR-01)                                                            | recovered by header (ERR-07) | settled with `drafts.get` (SND-08)       | retried                                                                       |
| 401                                              | refreshed once, then retried (ERR-05); `invalid_grant` gives Sign in again (ACC-08) | same                         | same                                     | same                                                                          |
| 429, 5xx, 403 rate-limit or backendError         | back off and retry, row unchanged, silent (ERR-01/02/03)                            | 5xx: recovery rule (ERR-07)  | 5xx: no clear answer (SND-08)            | back off                                                                      |
| 404 on a named item                              | that id is dropped, the rest retried (ERR-06)                                       | draft rules (DRF-04)         | 404 means sent (SND-08)                  | item dropped, silent (ERR-08); `history.list` 404 runs a full resync (SYN-04) |
| 409 on `labels.create`                           | –                                                                                   | –                            | –                                        | use the existing label (LAB-05)                                               |
| any other 4xx, including 403 with another reason | **rejection**: rollback and toast (ERR-04)                                          | rejection                    | rejection, the draft comes back (SND-06) | –                                                                             |

#### ERR-01 · P1 · Transient failures retry silently and keep the row as the user left it

_DT · §Gmail errors_

- **Steps:** For each outcome: `fault(batchModify, outcome, {times: 2})`, then `e` on `p1`. Advance the clock through the back-off, then `settle()`.
- **Outcomes:** 429; 500; 503; 403 `rateLimitExceeded`; 403 `userRateLimitExceeded`; 403 `backendError`; `noResponse`.
- **Expect, for each:**
  - `p1` stays out of Inbox the whole time.
  - No toast.
  - In the end `p1` is archived in Gmail, with one effective change.

#### ERR-02 · P2 · Back-off timing and `Retry-After`

_BVA · §Gmail errors (1 s doubling to 60 s, with jitter, or `Retry-After`)_

- **Steps and expect:**
  1. `fault(batchModify, 429 with Retry-After 7, {times: 1})`, then `e`. At `advance(6_900)` there is no retry. At `advance(100)` the request is retried.
  2. `fault(batchModify, 500, {times: 8})`, then `e`. The retry gaps grow roughly 1, 2, 4, 8 … seconds, within jitter bounds, and never exceed 60 s. The exact jitter distribution is left to unit tests.

#### ERR-03 · P2 · A quiet sync status after 30 s of retrying

_BVA · §Gmail errors_

- **Steps:**
  1. `fault(batchModify, 503, {times: ∞})`, then `e`.
  2. `advance(29_000)`.
  3. `advance(2_000)`.
  4. `clearFaults()`, then `settle()`.
- **Expect:**
  - After step 2, there is no status.
  - After step 3, A shows a quiet sync status (where it shows is Q1). Cached mail still opens.
  - After step 4, the status is gone and `p1` is archived.

#### ERR-04 · P1 · A rejection rolls the action back and shows Gmail's error

_DT · §Gmail errors, §Archive, trash, star, read_

- **Steps:** For each action: `fault(batchModify, status 400 "Bad request: x", {nth: 1})`, run the action, then `settle()`.

| Action                    | Rolled back to                       |
| ------------------------- | ------------------------------------ |
| `e` on `p1`               | `p1` back in Inbox, at its position  |
| `#` on `p2`               | `p2` back in Inbox, and not in Trash |
| `!` on `p3`               | `p3` back in Inbox                   |
| `s` on `p4`               | `p4` not starred                     |
| `u` on `p6`               | `p6` read                            |
| `l` Travel on `p5`        | `p5` without Travel                  |
| `h` on `own`              | `own` in Inbox, with no snooze       |
| `Shift+E` on `p1` in Done | `p1` back in Done                    |

- **For every row:** an error toast contains "Bad request: x", and other actions are unaffected.
- **Also:** 403 with reason `insufficientPermissions` is a rejection too, unlike the rate-limit 403s in ERR-01.

#### ERR-05 · P1 · An expired token is refreshed once, and the request is retried

_ST · §Gmail errors (401)_

- **Steps:** `expireAccessTokens(A)`, then `e` on `p1`, then `settle()`.
- **Expect:**
  - The token endpoint was called exactly once.
  - `batchModify` was retried and succeeded, and `p1` is archived in Gmail.
  - No toast, and no Sign in again.

#### ERR-06 · P1 · A 404 on part of an action drops only the missing messages

_EG · §Gmail errors (404 on a named item)_

- **Steps:**
  1. `deliver` a 3-message thread T. `poll()`.
  2. `gmail.deleteMessage(m2)`, with no poll.
  3. `e` on T, then `settle()`.
  4. Second partition: delete the whole thread in Gmail before `e`.
- **Expect:**
  - The first `batchModify` gets 404. Then there are `messages.get` calls with `format=minimal` for the ids, and a retry with m1 and m3 only.
  - m1 and m3 are archived. No toast. T stays out of Inbox.
  - In the second partition, the action finishes silently, and the next poll drops T.

#### ERR-07 · P1 · `drafts.create` with no clear answer never makes a duplicate draft

_DT · §Gmail errors (`drafts.create` rule)_

- **Steps:** For each row: set the fault, then in `p3` press `r`, type, `settle()`, type more, `settle()`.

| Fault on `drafts.create`                        | Expect                                                                                      |
| ----------------------------------------------- | ------------------------------------------------------------------------------------------- |
| `noResponse`, once (Gmail created it)           | One Gmail draft with `X-Ubermensch-Draft` equal to the local id, and later edits update it. |
| `drop`, once (not created)                      | It is created on retry. One draft.                                                          |
| `statusAfterApply(500)`, once                   | One draft.                                                                                  |
| `noResponse`, twice (two copies exist in Gmail) | The app keeps one and deletes the extra copy. One draft.                                    |

- **Also, for every row:** Drafts lists a single row.

#### ERR-08 · P2 · A 404 while fetching a body is dropped silently

_EG · §Gmail errors (404 on a sync, search, or body fetch)_

- **Steps:** `deliver` thread T, and `hold` its `threads.get format=full` so the metadata is stored without the body. Delete T in Gmail. Release the hold. Open T.
- **Expect:**
  - No toast.
  - The app stays usable, and T is dropped from the list after the 404.

#### ERR-09 · P3 · A rejected undo or label create

_EG · §Gmail errors_

| Action                | Fault                           | Expect                                                              |
| --------------------- | ------------------------------- | ------------------------------------------------------------------- |
| `e` on `p1`, then `z` | 400 on the undo's `batchModify` | `p1` returns to archived, and an error toast shows.                 |
| create label "Bad"    | 400 on `labels.create`          | The label disappears and the threads lose it. An error toast shows. |

### 5.19 `sync.spec.ts` (SYN)

#### SYN-01 · P1 · The connect sync matches Gmail

_DT · §Sync · Done when 2_

- **Seed:** `standard` on a fresh database. For the drafts row, use `drafts`.

| Gmail holds                             | The app shows                                                                                       |
| --------------------------------------- | --------------------------------------------------------------------------------------------------- |
| every Inbox thread                      | Inbox lists exactly those threads (ids equal Gmail's `INBOX` threads), newest first                 |
| drafts                                  | Drafts lists each one                                                                               |
| `sent10` and `sent89`                   | listed in Sent                                                                                      |
| `sent91`                                | not listed until a search finds it                                                                  |
| `starredOld`                            | listed in Starred                                                                                   |
| `archivedOld`, `projOld*`, `invoiceOld` | not listed anywhere until a search or a label view finds them                                       |
| user labels                             | all in the `l` picker                                                                               |
| `sendAs`                                | The From picker lists both of A's addresses. B's display name comes from its `sendAs` (see ACC-02). |

- **Also:** the request log has no `attachments.get`.

#### SYN-02 · P1 · Mail that changed while the app was closed shows after relaunch

_ST · §Sync · Done when 2_

- **Steps:**
  1. `settle()`, then stop the app.
  2. While it is down: `deliver` a new thread N, and a reply into `p1`. Archive `p2` from another client, and star `p3`.
  3. Relaunch, then `settle()`.
- **Expect:**
  - Inbox equals Gmail's Inbox.
  - N is present, and `p1` is at the top with the reply.
  - `p2` is gone from Inbox and is in Done, because it had been seen in Inbox.
  - `p3` is starred.

#### SYN-03 · P1 · Polls bring in changes made elsewhere

_DT, BVA · §Sync_

- **Steps:**
  1. Make these changes from another client: archive `p1`; star `p2`; mark `p3` read; add Travel to `p4`; deliver a new message into `p5`; trash `p6`.
  2. `advance(44_000)`.
  3. `advance(1_000)`.
  4. Second partition: `gmail.focus(false)`, make a change, `gmail.focus(true)`.
- **Expect:**
  - After step 2, there is no `history.list` and the UI is unchanged. This needs H13b.
  - After step 3, there is a `history.list`, and the UI shows every change: `p1` in Done, `p2` starred, `p3` read, `p4` in Travel, `p5` at the top and unread, `p6` in Trash.
  - In the second partition, a `history.list` runs on focus without advancing the clock.

#### SYN-04 · P1 · History too old: a full resync that sends no notifications

_ST · §Sync (full resync steps)_

- **Steps:**
  1. Snooze `p6` locally.
  2. `expireHistory(A)`.
  3. From another client: `deliver` 5 new Inbox messages with `poll: false`, delete thread `p4` permanently (`deleteThread`), archive `p5`, and add `INBOX` to `p6`.
  4. `poll()`, then `settle()`.
  5. Afterwards, `deliver` 1 new message, then `poll()`.
- **Expect:**
  - The log shows `getProfile`, then `threads.list` with `includeSpamTrash=true` and `maxResults=500`.
  - Inbox equals Gmail's Inbox.
  - `p4` is dropped after `threads.get format=minimal` returns 404.
  - `p6` has lost its snooze and is in Inbox.
  - `notifications()` gained nothing for the 5 new messages.
  - In step 5, the new message is notified as usual: polling resumed.

#### SYN-05 · P3 · The full resync keeps a thread that moved during the listing

_EG · §Sync (step 3)_

- **Steps:** During the resync, make the `threads.list` pages skip `p2`, using the H8c query matcher. `threads.get format=minimal` for `p2` still returns 200.
- **Expect:** `p2` is kept.

#### SYN-06 · P2 · `sendAs` and labels refresh on focus

_EP · §Sync_

- **Steps:** `gmail.sendAs(A, "ana@new.dev", "Ana New")` and `gmail.label(A, "Fresh")`. Then `gmail.focus(false)` and `gmail.focus(true)`.
- **Expect:**
  - The From picker lists `ana@new.dev`.
  - The `l` picker lists Fresh.

#### SYN-07 · P3 · An opened thread fetches its bodies right away

_EP · §Sync (priority 2)_

- **Seed:** `bulk` at the production quota budget, immediately after connect.
- **Steps:** Open the 50th row before its body has been fetched.
- **Expect:** its body renders within a few seconds, while other Inbox bodies are still pending.

#### SYN-08 · P1 · A slower thread fetch never undoes a newer one

_EG · §Sync, §Data_

- **Steps:**
  1. `holdAnswer` on the next `threads.get format=full`. `deliver` a new thread "Race", so its body fetch reads Gmail and its answer parks. Then `deliver` a reply into it (the poll stores it), `settle()`, and release.
  2. The same with a new thread "Late read": while its body answer is parked, mark it read from another client with `poll: true`, `settle()`, then release.
- **Expect:**
  - "Race" opens with both messages: the stale answer deletes nothing.
  - "Late read" stays read.

#### SYN-09 · P2 · A connect sync retried after a failure misses no change made in between

_EG · §Sync (connect)_

- **Seed:** `standard`, not opened.
- **Steps:** Fault A's next `drafts.list` with a 500, then open the app: the connect sync lists Inbox and fails at the drafts. Archive `p1` from another client with no poll, then `advance(10_000)` so the connect sync runs again.
- **Expect:** `p1` leaves Inbox, and Inbox equals Gmail's: polling resumes from the first attempt's cursor, not a newer one.

### 5.20 `notifications.spec.ts` (NOT)

When a new message raises a notification (§Notifications):

| Condition                                                                               | Notify?                  |
| --------------------------------------------------------------------------------------- | ------------------------ |
| `history.list` reports it added with `INBOX` and `UNREAD`, and the account is not muted | yes, naming the account  |
| … and its From is one of the connected accounts' addresses                              | no                       |
| … and the focused window has that thread **open**                                       | no                       |
| … and that thread is open but the window is unfocused                                   | yes                      |
| … and that thread is only highlighted (prefetched) in the focused window                | yes                      |
| added without `UNREAD`                                                                  | no                       |
| added without `INBOX`                                                                   | no                       |
| found by the connect sync or a full resync                                              | no                       |
| the account is muted                                                                    | no                       |
| more than 3 such messages for one account in one poll                                   | one summary notification |

#### NOT-01 · P1 · New mail in either account raises a notification and adds to the Dock badge

_EP · §Notifications · Done when 11_

- **Seed:** `standard`. Read the starting badge: A's unread Inbox count plus B's.
- **Steps:** `deliver` an unread Inbox message to A (on screen) and one to B, each with `poll: false`. Then `poll()`.
- **Expect:**
  - Two notifications, one naming `ana@acme.test` and one naming `ben@bolt.test`.
  - `badge()` went up by 2.
  - Each avatar went up by 1.

#### NOT-02 · P1 · When a notification is not raised

_DT · §Notifications_

- **Steps:** One step per table row, with `clearRecorded()` before each.

| Step                                                         | Notifications | Badge                                            |
| ------------------------------------------------------------ | ------------- | ------------------------------------------------ |
| B sends to A (From `ben@bolt.test`)                          | none          | +1                                               |
| A has `p2` open, window focused; a reply arrives in `p2`     | none          | unchanged: the message is marked read on arrival |
| the same, window unfocused                                   | 1             | +1                                               |
| `p2` only highlighted in the focused window; a reply arrives | 1             | +1                                               |
| a message added without `UNREAD`                             | none          | unchanged                                        |
| a message added without `INBOX`                              | none          | unchanged                                        |
| add account C, whose Inbox has 3 unread                      | none          | +3                                               |

#### NOT-03 · P1 · More than three new messages in one poll become one summary

_BVA · §Notifications_

- **Steps:** Each row delivers with `poll: false`, then runs one `poll()`.

| Delivered in one poll | Expect                                                 |
| --------------------- | ------------------------------------------------------ |
| 3 to A                | 3 notifications                                        |
| 4 to A                | 1 summary notification for A                           |
| 4 to A and 2 to B     | 1 summary for A, plus 2 individual notifications for B |

#### NOT-04 · P1 · A muted account does not notify and is left out of the badge

_DT · §Notifications, §Settings_

- **Steps:**
  1. On A, run Command palette → "Mute notifications". Reopen the palette.
  2. `deliver` an unread message to A.
  3. Open Settings.
  4. Run "Unmute notifications".
  5. Mute B from Settings while A is on screen.
- **Expect:**
  - After step 1, the palette offers "Unmute notifications".
  - After step 2: no notification. A's avatar goes up by 1. `badge()` excludes A and equals B's unread count.
  - In step 3, A's Mute switch is on.
  - After step 4, `badge()` includes A again, and the next delivery to A notifies.
  - In step 5, the palette's mute command still acts only on A.

#### NOT-05 · P1 · The Dock badge counts unread Inbox threads

_DT, BVA · §Notifications_

| Change                               | `badge()`                        |
| ------------------------------------ | -------------------------------- |
| a thread with 3 unread messages      | counts as 1                      |
| open an unread thread (read on open) | −1                               |
| `u` on a read Inbox thread           | +1                               |
| archive an unread thread             | −1                               |
| snooze an unread thread              | −1                               |
| an unread thread that is in Done     | not counted                      |
| every Inbox thread read, and B muted | 0: the badge is cleared (`null`) |

#### NOT-06 · P2 · Clicking a notification opens that account's Inbox on the thread

_ST · §Notifications, DEVIATIONS (focus within 30 s counts as a click)_

- **Steps:**
  1. With A on screen, `gmail.focus(false)`. `deliver` U to B and `poll()`.
  2. Click the notification (or `gmail.focus(true)` within 30 s, per DEVIATIONS).
  3. Repeat with 4 messages, so the notification is a summary.
  4. Repeat with a touched composer open on A.
- **Expect:**
  - After step 2, B is on screen, in Inbox, with U highlighted.
  - The summary opens B's Inbox.
  - With the composer open, it is closed first and the draft is kept.
- **P3 part:** focusing again after more than 30 s does not switch accounts.

### 5.21 `settings.spec.ts` (SET)

#### SET-01 · P2 · What the Settings sheet holds, and how it opens

_EP · §Settings, §Keyboard (`?`)_

- **Steps:** Open Settings with `Cmd+,`, then with Command palette → "Settings". Then press `?`.
- **Expect:**
  - Both open the same sheet.
  - It has "Load remote images", on by default.
  - It has one block per account, with the address, a Mute notifications switch, **Split Inbox settings** (which opens that account's split sheet), and **Remove account**.
  - **Sign in again** shows only for an account that needs it (see ACC-08).
  - `Esc` closes the sheet.
  - `?` opens the shortcut sheet, and `Esc` closes it.

#### SET-02 · P1 · Remote images

_DT · §Reading, §Settings_

- **Seed:** `standard`. Open `links`.

| Setting          | Remote `<img>`        | CSS background image | `data:` image |
| ---------------- | --------------------- | -------------------- | ------------- |
| on (the default) | fetched (H20 hit log) | fetched              | shown         |
| off              | **not** fetched       | **not** fetched      | shown         |

- **Also:**
  - Changing the setting while `links` is open re-renders it at once, with no reopen.
  - The setting survives `restart()`.
  - It is not an undo entry (UND-03).

### 5.22 `palette.spec.ts` (PAL)

#### PAL-01 · P1 · Open, filter, run, and close the palette

_ST · §Keyboard · Done when 4_

- **Steps:**
  1. Press `Cmd+K`.
  2. Type "sta".
  3. Choose "Go to Starred".
  4. Press `Cmd+K`, type "e", then `Esc`.
- **Expect:**
  - The palette lists actions and views, each with its shortcut on the right ("Mark done" E, "Go to Sent" G T, and so on).
  - "sta" filters to "Star or unstar" and "Go to Starred".
  - Choosing "Go to Starred" shows Starred and closes the palette.
  - Step 4 closes the palette with nothing changed: `req(batchModify)` is empty.

#### PAL-02 · P2 · Palette commands do what their keys do

_DT · §Keyboard_

- **Steps:** Run each command once from the palette. Check that the result matches the key's case.

| Command                     | Same as                                           |
| --------------------------- | ------------------------------------------------- |
| Mark done                   | TRI-01                                            |
| Trash                       | TRI-03                                            |
| Star or unstar              | TRI-05                                            |
| Mark read or unread         | TRI-05                                            |
| Undo                        | UND-01                                            |
| Reply                       | REP-02                                            |
| Compose                     | NEW-01                                            |
| Search                      | SRC-01                                            |
| Go to Done                  | NAV-05                                            |
| Keyboard shortcuts          | SET-01                                            |
| Switch account              | ACC-03; it lists both accounts with `⌃1` and `⌃2` |
| Split Inbox settings        | SPL-06                                            |
| Settings                    | SET-01                                            |
| Mute / Unmute notifications | NOT-04                                            |

### 5.23 `security.spec.ts` (SEC)

#### SEC-01 · P1 · Hostile HTML cannot run script or navigate

_EG · §Reading, §Security · Done when 12_

- **Seed:** `security`. Open `hostile` with every message expanded (`Shift+O`).
- **Steps:**
  1. Click the `javascript:` link and the form's submit button.
  2. `Tab` to each link and press `Enter`.
  3. `advance(1_000)`.
- **Expect, script and dialogs:**
  - `window.__pwned` is undefined in the app page and in the iframe.
  - No `dialog` event fired.
- **Expect, navigation:**
  - The page URL is unchanged.
  - `openedUrls` and the popup log hold no `javascript:` URL and nothing at `evil.test`.
  - The H20 sink got no hits from `evil.test`.
- **Expect, the frame:**
  - The iframe has `sandbox="allow-same-origin allow-scripts"`.
  - Its document's head starts with a CSP `meta` containing `script-src 'none'`.
  - Its DOM has no `<script>`, no `on*` attribute, no `<form>`, `<iframe>`, or `<base>`, and no `meta refresh`.

#### SEC-02 · P1 · Mail text never becomes markup outside the frame

_EG · §Security, AGENTS ("Treat all mail content as hostile")_

- **Seed:** `security`, thread `hostileMeta`.
- **Steps:** Look at its list row and conversation subject. Pin its sender with "Move to Important". Reply and send it so its subject appears in the send toast. Type "Bo" in To so its name appears in autocomplete.
- **Expect:**
  - Each place shows the markup as literal text.
  - None of these places contains an injected `<img>` or `<b>` element.
  - `__pwned` is undefined.

#### SEC-03 · P1 · Links never navigate the app window

_DT · §Reading_

- **Seed:** `standard`. Open `links`.

| Link                      | How     | Expect                                            |
| ------------------------- | ------- | ------------------------------------------------- |
| `https://example.org/a`   | click   | recorded as opened, and the page URL is unchanged |
| `mailto:x@example.org`    | click   | recorded as opened                                |
| `target="_top"` link      | click   | recorded once, and the page is unchanged          |
| a link reached with `Tab` | `Enter` | recorded as opened                                |

#### SEC-04 · P2 · The webview holds no Gmail credentials

_EG · §Security_

- **Steps:** After the connect sync, search the browser's storage and the database for token strings.
- **Expect:**
  - No token string from `keychain()` appears in `localStorage`, `sessionStorage`, IndexedDB, or the JSON that the `window.__ub` stores serialize to.
  - Every request in the fake's log came from the core: no `Origin` header, and no browser user agent.
  - `inspect.dbContains(token)` is false.

### 5.24 `ui-consistency.spec.ts` (UIC)

#### UIC-01 · P1 · Rows you just acted on never flash back during fast triage or a poll

_EG · §UI state (Actions: "Rows the user just acted on never flash back during fast triage or a poll")_

- **Seed:** `bulk`. Install the H23 probe: a `MutationObserver` that records every row id added to the list.
- **Steps:**
  1. `hold(batchModify)`.
  2. Press `e` 10 times quickly.
  3. Halfway through, `deliver` 2 messages to other threads and `poll()`.
  4. `release`, then `settle()`.
- **Expect:**
  - No archived row id is ever added back to the DOM.
  - The final list equals Gmail's Inbox.
  - The highlight is on the row after the last archived one.
- **Second partition (the ack boundary):** park the page's submits with `page.route` and let them through one at a time. `e` on `p1`, then `e` on `p2`. Let the first submit go: the core applies and acks it while the second waits.
  - `p2` stays out of the list while the page has seen the ack for `p1` but the core has not yet applied `p2`.
  - After both go through, `p2` was never added back.

#### UIC-02 · P2 · A reload keeps queued actions and keeps action numbers going

_EG · §UI state (Actions: "a webview reload does not reuse numbers of actions still queued")_

- **Steps:**
  1. `offline(true)`. `e` on `p1`.
  2. `page.reload()`. `e` on `p2`.
  3. `offline(false)`, then `settle()`.
  4. Press `z`, then `z`.
- **Expect:**
  - Both `p1` and `p2` are archived in Gmail.
  - `z` undoes `p2`, then `p1`. Both entries survived the reload.

#### UIC-03 · P2 · An action whose submit fails is sent again

_EG · §UI state (Actions: "The UI sends actions one invoke at a time"); DEVIATIONS (a failed submit is sent again)_

- **Steps:** with `page.route`, answer the page's first `submit` with a 500 and let later ones through. `e` on `p1`, then `settle()`.
- **Expect:**
  - `p1` is archived in Gmail.
  - `p1` stays out of the list, also after a reload.

#### UIC-04 · P2 · A row that changed while no list held it is not shown from an old copy

_EG · §UI state (Subscriptions: "Closing a subscription drops its rows unless another subscription holds them")_

- **Seed:** `empty`, then 90 Inbox threads `Batch 01`–`Batch 90`, newest first, so `Batch 90` is past the rows a first load carries.
- **Steps:**
  1. Scroll to the bottom: `Batch 90` shows unread.
  2. Another client archives it (poll), then marks it read (poll).
  3. Search `Batch` and scroll to the bottom.
- **Expect:** `Batch 90` shows as read.

#### UIC-05 · P1 · `Cmd+Q` quits only once the composer's close has committed

_EG · §UI state (Actions: "`Cmd+Q` first closes an open composer as a second `Esc` would, and quits once that action commits")_

- **Steps:**
  1. Reply to `p3` and type a line; do not wait for the autosave.
  2. Park the page's submits with `page.route` and pause the page clock.
  3. Quit (`app.quit()`, not awaited). Once the close's submit is parked, run the page clock 2 s.
  4. Let the submit through, resume the clock, finish the quit, and relaunch.
- **Expect:**
  - While the close is parked, the page has not called `quit_now`: the submit is its only invoke in flight.
  - Gmail has one draft, with the typed line.

---

## 6. Done-when traceability

| #   | Done-when item (SPEC)                                                                                                               | Cases                                                                  |
| --- | ----------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------- |
| 1   | Two accounts can be added, switched with `Ctrl+1` and `Ctrl+2`, and removed. Tokens are in Keychain.                                | ACC-01, ACC-02, ACC-07 (the Keychain is the recording store, see §8)   |
| 2   | Each Inbox matches Gmail after a sync, including mail that arrived while the app was closed                                         | SYN-01, SYN-02                                                         |
| 3   | Split sections, `Tab` between them, a sender pinned to Important stays across syncs, empty splits hide except for `g` `i` / `g` `o` | SPL-01, SPL-02, SPL-04, SPL-08                                         |
| 4   | `j`, `k`, `e`, `h`, `c`, `r`, `Enter`, `/`, and `Cmd+K` do what the table says                                                      | NAV-02, NAV-03, TRI-01, SNZ-01, NEW-01, REP-01, REP-02, SRC-01, PAL-01 |
| 5   | Every performance budget holds                                                                                                      | **Not E2E** (§8)                                                       |
| 6   | Archive, label, star, trash, and snooze update the list before the network returns, and show up in Gmail                            | TRI-01, LAB-01, TRI-05, TRI-03, SNZ-03 (each uses `hold`)              |
| 7   | A long thread opens collapsed and scrolled to the first unread                                                                      | CONV-01                                                                |
| 8   | Reply opens under the thread; autocomplete suggests people this account has written to                                              | REP-01, RCP-01, RCP-02                                                 |
| 9   | A send can be undone from the toast; a failed send retries from the outbox                                                          | SND-01, SND-02, SND-07, SND-08                                         |
| 10  | Airplane mode: cached threads open, search finds cached mail, an archive or a reply lands later                                     | OFF-01, SRC-05, SND-07                                                 |
| 11  | New Inbox mail in either account raises a notification and increments the Dock badge                                                | NOT-01                                                                 |
| 12  | Hostile HTML cannot run script in the app window                                                                                    | SEC-01, SEC-02                                                         |

## 7. SPEC section traceability

| SPEC section                | Cases                                                                                                                                                                       |
| --------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Window                      | NAV-01, NAV-03, NAV-04, NAV-06, SPL-10 (native window behavior: §8)                                                                                                         |
| Look, Motion                | ACC-02 (avatar hue, P3 detail); the rest are in §8                                                                                                                          |
| Performance                 | §8                                                                                                                                                                          |
| Views                       | VIEW-01 to VIEW-05, NAV-05, SRC-07                                                                                                                                          |
| Accounts, Switching         | ACC-01 to ACC-10, SND-10, NEW-10                                                                                                                                            |
| Keyboard                    | NAV-02 to NAV-05, NAV-07, TRI-02, TRI-06, TRI-07, TRI-09, NEW-05 to NEW-07, NEW-12, PAL-01, PAL-02                                                                          |
| Undo                        | UND-01 to UND-08, TRI-06, SPL-04, SND-03, SNZ-07                                                                                                                            |
| Reading                     | CONV-01 to CONV-14, DRF-02, ATT-02, SEC-01, SEC-03, SET-02                                                                                                                  |
| Compose                     | NEW-02 to NEW-04, NEW-08, NEW-11                                                                                                                                            |
| Reply composer              | REP-01, REP-02, REP-07 to REP-10, NEW-01                                                                                                                                    |
| Addressing                  | REP-03 to REP-06, REP-11, REP-12, ATT-03, NEW-10                                                                                                                            |
| Recipients and autocomplete | RCP-01 to RCP-08, NEW-09                                                                                                                                                    |
| Drafts                      | DRF-01 to DRF-09, ATT-01, ATT-04, NEW-11, VIEW-04                                                                                                                           |
| Send and undo               | SND-01 to SND-12, REP-10, ATT-03                                                                                                                                            |
| Archive, trash, star, read  | TRI-01 to TRI-11, ERR-04                                                                                                                                                    |
| Labels                      | LAB-01 to LAB-07                                                                                                                                                            |
| Split Inbox                 | SPL-01 to SPL-10                                                                                                                                                            |
| Snooze                      | SNZ-01 to SNZ-07, SYN-04, TRI-08                                                                                                                                            |
| Settings                    | SET-01, SET-02, ACC-07, NOT-04, UND-03                                                                                                                                      |
| Search                      | SRC-01 to SRC-07                                                                                                                                                            |
| Offline                     | OFF-01 to OFF-07, LAB-04, DRF-05, DRF-06, SRC-05                                                                                                                            |
| Notifications               | NOT-01 to NOT-06, CONV-09, SYN-04                                                                                                                                           |
| Sync                        | SYN-01 to SYN-07, LAB-07, VIEW-05                                                                                                                                           |
| Gmail errors                | ERR-01 to ERR-09, ACC-08, SND-05 to SND-08, LAB-05, DRF-04, OFF-05                                                                                                          |
| Data                        | ACC-07, ACC-10, SEC-04                                                                                                                                                      |
| UI state                    | TRI-10, UND-05, UIC-01 to UIC-05, DRF-08, CONV-03, CONV-09, NEW-12                                                                                                          |
| Security                    | SEC-01 to SEC-04, ACC-01                                                                                                                                                    |
| DEVIATIONS                  | TRI-05 (`u` on the latest message only), NEW-07 (an undo-reopened draft is never discarded), NOT-06 (click heuristic), SNZ-04 (wake detection), SYN-02 (re-list on restart) |

---

## 8. Not covered end-to-end, and why

| Spec behavior                                                                                                                                                     | Why it is not an E2E case                                                                                                                                                                           | Where it is covered instead                                                                                                  |
| ----------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| Window behavior: a second launch focuses the window, closing hides it, a Dock click shows it, size and position are restored, the overlay title bar, drag regions | The native shell is not exercised.                                                                                                                                                                  | Manual Done-when pass on a real Mac                                                                                          |
| Real Keychain and real Google OAuth: system browser, the unverified-app warning, a consent screen in production                                                   | The fake OAuth and a memory token store stand in for them. ACC-01 checks the token is stored under `{service: "ubermensch", account: email}`, and checks scope, PKCE, and loopback in the auth URL. | Manual pass against two real Google accounts (SPEC Testing)                                                                  |
| Performance budgets, the perf fixture, the keydown-to-paint meter, 60 fps scrolling, cold launch time                                                             | Timing on CI WebKit is not the M1 WKWebView target.                                                                                                                                                 | The separate perf check (SPEC Testing, Performance)                                                                          |
| Look (colors, type sizes, spacing, the palette's dark theme), Motion, Reduce Motion                                                                               | Visual, not behavioral. Optional screenshot checks could be added later.                                                                                                                            | Design review                                                                                                                |
| The quota bucket (1,000 units, refill 4,800 a minute, the 4,000 a minute background cap), the six priority levels, at most 8 requests per account                 | Internal scheduling. Through the UI it is neither observable nor stable (SYN-07 is the one smoke check).                                                                                            | `crates/ub-gmail/tests/scheduler.rs` (kept; see §9). The on-screen account going first has no observable effect (DEVIATIONS) |
| The exact back-off jitter distribution                                                                                                                            | Random. ERR-02 checks `Retry-After` and that gaps grow.                                                                                                                                             | `backoff_doubles_to_a_minute_with_jitter` in `scheduler.rs` (kept)                                                           |
| OAuth protocol edge cases: a wrong `state`, concurrent 401s sharing one refresh, refresh before expiry, secrets kept out of `Debug` output                        | Below the UI. A token leaking into logs is not observable E2E.                                                                                                                                      | Keep `crates/ub-gmail/tests/oauth.rs` (recommended)                                                                          |
| The classification of each HTTP response into an outcome                                                                                                          | ERR-* and SND-08 cover the **behavior** for each outcome. The mapping itself is implied.                                                                                                            | –                                                                                                                            |
| Sanitizer policy, element by element                                                                                                                              | ammonia's allow-list. SEC-01 checks a representative hostile sample end to end.                                                                                                                     | Kept `ub-mime/tests/sanitize.rs`                                                                                             |
| MIME parsing robustness: charsets, malformed headers, random inputs                                                                                               | Fuzzing does not belong in E2E. ATT-04 has one encoded-subject round trip.                                                                                                                          | Kept `ub-mime/tests/no_panic.rs`                                                                                             |
| The query language grammar, its error messages, FTS translation, Unicode matching                                                                                 | Pure parsing. SRC-02 and SPL-06 check one representative per operator family, plus the wiring: body words only in search, `is:list` stripped for Gmail, unsupported operators ignored locally.      | Kept `ub-rules` `query.rs` tests                                                                                             |
| Snooze arithmetic across weekdays and DST, the phrase grammar                                                                                                     | Pure. SNZ-01 checks one normal hour and SNZ-02 one boundary.                                                                                                                                        | Kept `ub-rules` `snooze.rs` tests                                                                                            |
| Contact score math: recency halving every 90 days, count × recency, word-start matching rules                                                                     | Pure. RCP-02 checks tier order and exclusions from real ingest.                                                                                                                                     | Kept `ub-rules` `contacts.rs` tests                                                                                          |
| Person-to-person classification over every label id combination                                                                                                   | Pure. SPL-01 checks the list-header flag at ingest and four categories.                                                                                                                             | Kept `ub-rules` `splits.rs` `person_to_person`                                                                               |
| Writer and channel ordering: `rev`, `ack`, patches against first loads, a stale fetch never bringing a row back                                                   | Races that cannot be reproduced deterministically through the UI. UIC-01 checks only the user-visible no-flash property.                                                                            | Kept `tests/writer_channel.rs` proptest                                                                                      |
| Fine detail of the mirror's pending-action re-application (the deleted `mirror-core.test.ts`)                                                                     | Covered at the behavior level by UND-05, UIC-01, TRI-01, and TRI-10. The exact `ack` interleavings are not reproducible E2E.                                                                        | Consider keeping a small Vitest for `undoTarget` and the removal table if UND-05 proves flaky.                               |
| Table schemas, `rev` and action-number values, `contact_sources` dedup (each message id counts once)                                                              | Internal. Double counting after a resync would only show as a ranking shift, which is not observable with confidence.                                                                               | Consider one unit or integration test of contacts ingest idempotence.                                                        |
| A due snooze not yet woken (app closed, Mac asleep) "stays in Snoozed and out of Done"                                                                            | Only true while the app is not running, so the UI cannot show it. SNZ-04's launch row checks that Gmail stays out of Inbox while the app is down and that the thread wakes before first paint.      | –                                                                                                                            |
| The full resync step-3 race at scale; billing and units-per-day figures                                                                                           | Cost and scale facts                                                                                                                                                                                | –                                                                                                                            |
| Real sleep and wake events                                                                                                                                        | Simulated by H14                                                                                                                                                                                    | –                                                                                                                            |
| Label colors, editing nested labels, permanent delete, other non-goals                                                                                            | Out of scope in SPEC                                                                                                                                                                                | –                                                                                                                            |

## 9. Kept unit and property tests

These stay, and the E2E suite deliberately does not repeat their partitions.

| Test                                                    | What it covers                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   | E2E overlap, kept small on purpose                                                                       |
| ------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------- |
| `src-tauri/crates/ub-rules/src/query.rs` (inline tests) | Lenient and strict parsing, error messages (`Missing )`, `OR needs a term after it`, …), operator case-insensitivity, AND binding tighter than OR, parentheses, quoted phrases, unsupported operators kept as always-true terms, rule mode against search mode (bare words search the body only in search), the `label:` hyphen form and case, FTS5 translation, escaping, prefix only on the last word, valid SQLite output, `strip_is_list`                                                                                                    | SRC-02 (one query per family, plus `is:list` stripped from `q`) and SPL-06 (label case, `CATEGORY_` ids) |
| `ub-rules/src/splits.rs`                                | `person_to_person` over category and list combinations; the starters parse and match                                                                                                                                                                                                                                                                                                                                                                                                                                                             | SPL-01, SPL-07                                                                                           |
| `ub-rules/src/snooze.rs`                                | Options before and after 18:00, the next calendar day after midnight, next week on every weekday, DST gaps and folds, the phrase grammar (`fri 9am`, `in 2 hours`, `dec 24`, ISO dates, `tmrw`), rejected inputs                                                                                                                                                                                                                                                                                                                                 | SNZ-01, SNZ-02                                                                                           |
| `ub-mime/tests/sanitize.rs`                             | The hostile fixture: scripts, handlers, `javascript:`, forms, iframes, svg, math, meta refresh, base, link, `data:text/html` removed; style, class, id, `data:image`, remote images, legacy table attributes kept; links get `rel=noopener`; `cid:` rewritten to `data:` before sanitizing; `data:` allowed only for images; text extraction leaves out hidden content                                                                                                                                                                           | SEC-01 (the integration: sanitized at ingest, sandbox, CSP)                                              |
| `ub-mime/tests/no_panic.rs`                             | 3,000 seeded random inputs through every parser and the sanitizer; no panic                                                                                                                                                                                                                                                                                                                                                                                                                                                                      | none                                                                                                     |
| `src-tauri/tests/writer_channel.rs`                     | Proptest: random subscribes, actions (Done, Return to inbox, trash, spam, star, read, snooze), undo, and remote `INBOX` changes. A list rebuilt from its first load plus patches always equals a fresh load at that `rev`. Also: a reloaded page's action numbers continue after actions still queued behind a running job, not after the last committed one (`launch_numbers_continue_after_actions_still_queued`); and a first load lost to a failed commit goes out with the next one (`a_first_load_lost_to_a_failed_commit_is_sent_again`). | UIC-01 (only the user-visible part)                                                                      |

Also kept, because they test logic the E2E suite can't observe:

- `crates/ub-gmail/tests/scheduler.rs`: priorities, concurrency, quota, and back-off.
- `crates/ub-gmail/tests/oauth.rs`: the `state` check, token caching and early refresh, one refresh shared by concurrent 401s, and secrets kept out of debug output.
- `src-tauri/src/clock.rs`: the activity tracking `settle()` relies on, and that closing a `Kick` (an account stopping) wakes a loop even between its last check and its wait.
- `src-tauri/src/db/mod.rs`: older databases get `AUTOINCREMENT` on `notices` and `outbox` without losing rows.

Everything else was deleted, because a case here covers it: the contact ranking (RCP-02), the MIME build and Gmail payload tests (ATT-04, CONV), the address, encoding, quote and HTML helpers, the list diff (UIC-01), the mirror's Vitest tests (UIC, UND), `ub-gmail/tests/{api,outcomes}.rs` (ERR), and `src-tauri/tests/{sync_fake,compose_fake,splits}.rs` (SYN, SND, DRF, SPL).

---

## 10. Decisions on open questions

The design agent listed 26 places where the SPEC is silent or ambiguous. Each is decided below, following `AGENTS.md`: take the closest reading of the SPEC, and record real departures in `DEVIATIONS.md`. Cases assert these decisions.

| Q                                    | Decision                                                                                                                                                             |
| ------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Q1 Quiet sync status                 | Build it: the rail avatar gets a muted marker and its title appends ` · Can't reach Gmail` after 30 s of retrying; it clears on the next success. ERR-03 asserts it. |
| Q2 Sign in again                     | Both places: the rail marker with its title, and the button in the account's Settings block.                                                                         |
| Q3 Reply From with two own addresses | The address in To wins over Cc; within a field, the first one.                                                                                                       |
| Q4 Untouched                         | Judged by value: typing then deleting back to the start leaves the draft untouched, so it is discarded.                                                              |
| Q5 Prefixes                          | Replies get `Re:` and forwards `Fwd:`, unless the subject already starts with that prefix.                                                                           |
| Q6 Held undo meets no clear answer   | The undo wins: if `drafts.get` finds the draft still there, it is not sent again and returns as "not sent".                                                          |
| Q7 Notify after relaunch             | Yes. Only the connect sync and a full resync are silent.                                                                                                             |
| Q8 Remove account offline            | Revocation is best effort and skipped offline. Not asserted.                                                                                                         |
| Q9 Split settings                    | Saving the sheet is an undo entry (§Undo: "changes mail or splits"). The Save button stays; recorded in DEVIATIONS.                                                  |
| Q10 Action in its own view           | A no-op: no undo entry, no Gmail call, no toast.                                                                                                                     |
| Q11 Ends of lists                    | Clamp: the highlight or selection stays put.                                                                                                                         |
| Q12 What search restores             | `Esc` restores the view and its highlight. An account switch that closed search returns to that account's last view.                                                 |
| Q13 Snooze date and time             | The typed phrase is the date-and-time choice. Recorded in DEVIATIONS.                                                                                                |
| Q14 401 after a refresh              | Sign in again.                                                                                                                                                       |
| Q15 `Shift+J/K` reversal             | Not asserted.                                                                                                                                                        |
| Q16 Notification click               | Assert the focus heuristic through `gmail.focus()`, plus a web equivalent of `notification-open` (H18).                                                              |
| Q17 `999+` on avatars                | Not asserted.                                                                                                                                                        |
| Q18 `Ctrl+N` while composing         | Works; switching closes the composer (§Switching).                                                                                                                   |
| Q19 Label pins on rename             | Keyed by name; a rename does not carry the pin. Not asserted.                                                                                                        |
| Q20 Create-and-apply undo            | `z` removes the label from the threads and keeps the label. A rejected `labels.create` removes the label row.                                                        |
| Q21 Removal checks                   | UI-visible effects plus the revocation log only; no database inspection.                                                                                             |
| Q22 Empty split, hide off            | Its name shows in the list header with no count; no section header in the list.                                                                                      |
| Q23 Notification account             | The email address.                                                                                                                                                   |
| Q24 Gmail edit vs composer edit      | Last writer wins: the next upsert overwrites Gmail's version.                                                                                                        |
| Q25 Star scope                       | Asserted at thread level only.                                                                                                                                       |
| Q26 Send with no recipients          | Refused with an "Add a recipient" toast; nothing is sent, the draft stays. Recorded in DEVIATIONS.                                                                   |

## 11. Selector notes and testability gaps

What the UI exposes today. `e2e/app.ts` already wraps some of these.

| Area                     | Selectors                                                                                                                                                                                                                                                       |
| ------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| List                     | `role=listbox[name="Conversations"]`. Rows are `role=option` with `id=row-<threadId>` and `data-row`. The highlight is `aria-selected="true"`, and the listbox's `aria-activedescendant` points at it.                                                          |
| Split headers and names  | Split headers are `[data-split=<id>]`. Split names in the list header are `<button>`s.                                                                                                                                                                          |
| Empty states             | "You're all done." / "Inbox zero." in Inbox, "Nothing here." elsewhere.                                                                                                                                                                                         |
| Conversation             | The subject is `h1`. Collapsed messages are `button[data-message][aria-expanded=false]`. An expanded message is `[data-message]` with a header `button[aria-expanded=true]`. A fold is a button with the text "N older messages".                               |
| Message bodies           | `iframe[data-frame=<messageId>]`, titled `message <id>`. "Loading…" or "Downloading message…" shows before a body arrives.                                                                                                                                      |
| Pending sends and drafts | A pending send is a row with "Sending" or "Email delayed". A draft stub is a button with "Draft".                                                                                                                                                               |
| Top bar                  | Buttons titled "Back (Esc)", "Previous conversation (K)", "Next conversation (J)", and "Search (/)". The search input has the placeholder "Search", and the offline note reads "Results from this Mac".                                                         |
| Rail                     | Buttons titled "Inbox", "Done (g e)", "Snoozed (g h)", "Starred (g s)", "Sent (g t)", and "Drafts (g d)". Avatars are buttons titled `<email> (Ctrl+N)`, with ` · Sign in again` appended when needed.                                                          |
| Palette                  | `dialog[aria-label=<placeholder>]`: "Type a command", "Label as…", "Snooze until… (try “fri 9am”)", "Go to label", "Switch account", "Add to split", "Move to Important", "Move to Other", "Send from", "Draft actions". Items are cmdk `[cmdk-item]`.          |
| Sheets                   | `dialog[aria-label]`: "Settings", "Keyboard shortcuts", or "Split Inbox · <email>". Switches are `role=switch`, named "Load remote images", "Mute notifications", or "Turn <name> on or off". Remove account asks to confirm with "Remove" or "Cancel".         |
| Composer                 | `role=combobox[name=To                                                                                                                                                                                                                                          | Cc  | Bcc]`, suggestions in `role=listbox#To-suggestions`with options, the Subject placeholder "Subject", the editor`.ProseMirror`, and the "New Message" heading on the `c`screen. Buttons: **Send**, attach titled "Attach (⌘⇧A)", discard titled "Discard (⌘⇧,)", the quote toggle titled "Show quoted text", and the From button (only with more than one`sendAs`). |
| Toasts                   | Sonner `[data-sonner-toast]`. Send toasts read "Sending…", "Sent", "Email delayed", or "Undoing…", with an **Undo** action. Other toasts: **Open draft**, "Draft discarded", "Draft was sent or deleted elsewhere", "Your message was not sent", "Send undone". |
| First run                | The button "Add a Gmail account".                                                                                                                                                                                                                               |

**Handles added for the cases above** (H18, H19, and H24 in section 3; the gaps this list named are closed):

1. Rows carry `data-selected` (selected with `x`), `data-unread`, and `data-starred` (`app.selected`, `app.unread`). The selected message has `aria-current="true"` (`app.selectedMessage`). Recipient chips are `[data-chip=<address>]`, invalid ones with `aria-invalid="true"` (`app.chips`, `app.invalidChips`); their remove buttons are named `Remove <address>`, as are the attachment chips' (`Remove <filename>`).
2. Split section headers are `h2` headings named `<name> <count>` (`app.section`). The split names in the list header are buttons in a `toolbar` named "Splits"; the active one has `aria-current="true"` (`app.splitTitles`, `app.activeSplit`, `app.splitHeader()`, `app.sections()`). The `c` screen's "New Message" is an `h1`.
3. The quiet sync status (Q1): after 30 s of retrying, the avatar shows `[data-marker="retrying"]` and its title ends in ` · Can't reach Gmail` (`app.syncMarker`). Sign in again is `[data-marker="signin"]` (`app.signInMarker`).
4. In web mode the page listens for `notification-open` and `quit-requested` as named server-sent events, and message links go to the core's `open_url`, so `openedUrls` records them.
5. The fake's sign-in signs in "the first user" unless `nextSignIn` names an address, once per sign-in.
