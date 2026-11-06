// A typed client for the E2E server's `/__test` control API
// (src-tauri/src/engine/testserver.rs).

export type Attachment = {
  filename: string;
  mimeType?: string;
  /** Text content, or base64 with `base64: true`. */
  content: string;
  base64?: boolean;
  /** Makes it an inline part, referenced from HTML as `cid:<contentId>`. */
  contentId?: string;
};

export type Delivery = {
  /** The mailbox the message lands in. */
  account: string;
  from: string;
  /** Default: the mailbox's address. */
  to?: string[];
  cc?: string[];
  subject: string;
  text?: string;
  html?: string;
  /** The charset the body parts are labelled with; their bytes stay UTF-8. Default: utf-8. */
  charset?: string;
  /** Gmail label ids. Default: INBOX, UNREAD. */
  labels?: string[];
  /** Joins this Gmail thread. */
  threadId?: string;
  inReplyTo?: string;
  references?: string;
  listId?: string;
  headers?: [string, string][];
  attachments?: Attachment[];
  /** Unix ms. Default: app time now. */
  date?: number;
  /** In `deliverMany`: joins the thread of this earlier item and replies to it. */
  after?: number;
  /** Poll at once so the app sees it without waiting. Default true. */
  poll?: boolean;
};

/** Gmail ids of a stored message; `messageId` is its `Message-ID` header. */
export type Delivered = { id: string; threadId: string; messageId: string };

export type GmailMessage = {
  id: string;
  threadId: string;
  labels: string[];
  date: number;
  snippet: string;
  /** Lower-cased header names; the first of repeated headers. */
  headers: Record<string, string>;
  /** The raw RFC 822 message (see `parseMime`). */
  raw: string;
};

export type GmailState = {
  messages: GmailMessage[];
  drafts: { id: string; message: GmailMessage }[];
  labels: { id: string; name: string }[];
  sendAs: { email: string; displayName: string; primary: boolean }[];
  historyId: number;
};

/** A draft made in Gmail by another client (web, a phone). */
export type DraftSpec = {
  /** Default: the mailbox's address. */
  from?: string;
  to?: string[];
  cc?: string[];
  subject?: string;
  text?: string;
  html?: string;
  threadId?: string;
  inReplyTo?: string;
  /** Default: `inReplyTo`. */
  references?: string;
  headers?: [string, string][];
  attachments?: Attachment[];
  /** Unix ms. Default: app time now. */
  date?: number;
};

export type AddedDraft = {
  id: string;
  messageId: string;
  threadId: string;
  rfc822MessageId: string;
};

/** Gmail API methods as the request log and faults name them. */
export type ApiMethod =
  | "getProfile"
  | "sendAs.list"
  | "labels.list"
  | "labels.create"
  | "history.list"
  | "threads.list"
  | "threads.get"
  | "messages.batchModify"
  | "messages.get"
  | "messages.trash"
  | "messages.send"
  | "filters.list"
  | "filters.create"
  | "filters.delete"
  | "attachments.get"
  | "drafts.list"
  | "drafts.create"
  | "drafts.get"
  | "drafts.update"
  | "drafts.delete"
  | "drafts.send"
  | "oauth.authorize"
  | "oauth.token"
  | "oauth.revoke";

/** What a matched request suffers. */
export type Outcome =
  /** The connection closes before anything is done. */
  | { kind: "drop" }
  /** The work is done, then the connection closes unanswered. */
  | { kind: "noResponse" }
  /** A Google error, nothing done. `reason` defaults by status. */
  | { kind: "status"; status: number; reason?: string; retryAfterSecs?: number }
  /** The work is done, then a Google error is returned. */
  | { kind: "statusAfterApply"; status: number; reason?: string }
  /** Wall-clock delay, then handled normally. */
  | { kind: "delay"; delayMs: number }
  /** Handled, then these Gmail thread ids are left out of the `threads.list` page. */
  | { kind: "omitThreads"; threads: string[] };

/** Picks requests. Every field given must match. */
export type Match = {
  api?: ApiMethod;
  /** HTTP method. */
  method?: string;
  /** Glob on the request path, e.g. `*\/messages/batchModify`. */
  path?: string;
  /** The account the request acts for (from its token). */
  account?: string;
  /** Query parameters that must be present, e.g. `{ format: "full" }`. */
  query?: Record<string, string>;
  /** Only the nth matching request (1-based, counted from injection). */
  nth?: number;
  /** Only the first n matching requests. Default: all of them. */
  times?: number;
};

/** A request the fake received. */
export type LoggedRequest = {
  /** Arrival order, from 0. */
  seq: number;
  api: ApiMethod | "unknown";
  method: string;
  path: string;
  /** The raw query string. */
  query: string;
  /** Query parameters; repeated keys are joined with commas. */
  params: Record<string, string>;
  account: string | null;
  /** The JSON body, if any. */
  body: unknown;
  /** App time when it arrived, Unix ms. */
  at: number;
  /** Parked by a hold right now. */
  held: boolean;
  /** HTTP status, `dropped`, `noResponse`, or null while unanswered. */
  answer: number | "dropped" | "noResponse" | null;
  origin: string | null;
  userAgent: string | null;
};

/** A parked request's hold; `release` lets them go. */
export type Hold = {
  id: number;
  /**
   * Ends the hold and lets what it parked go on: handled normally, or
   * suffering `outcome`. First waits until it has parked `min` requests
   * (default 1), so the release never overtakes its request. Returns how many.
   */
  release: (outcome?: Outcome, opts?: { min?: number }) => Promise<number>;
};

export type Notification = {
  title: string;
  /** The account's address, then the text. */
  body: string;
  /** The account's address, which the notification names (SPEC Notifications). */
  account: string;
  /** Gmail thread id; null for a summary. */
  thread: string | null;
};

export type Recorded = {
  notifications: Notification[];
  /** The Dock badge; null when cleared. */
  badge: number | null;
  openedUrls: string[];
  openedFiles: string[];
  focusRequests: number;
};

export type TokenItem = { account: string; token: string };

/** Where the page's channel should be, from the core's side of `settle()`. */
export type CoreIdle = { rev: number; ack: number; signIns: number; held: number };

export class Control {
  readonly url: string;

  constructor(url: string) {
    this.url = url;
  }

  private async call<T>(method: "GET" | "POST", path: string, body?: unknown): Promise<T> {
    const init: RequestInit =
      body === undefined
        ? { method }
        : { method, headers: { "content-type": "application/json" }, body: JSON.stringify(body) };
    const r = await fetch(`${this.url}${path}`, init);
    const data: unknown = await r.json();
    if (!r.ok) throw new Error(`${method} ${path}: ${JSON.stringify(data)}`);
    // The control API is our own server; each method names the shape its handler returns.
    // oxlint-disable-next-line typescript/no-unsafe-type-assertion
    return data as T;
  }

  private mailbox(email: string, rest = ""): string {
    return `/__test/gmail/${encodeURIComponent(email)}${rest}`;
  }

  // ---------------------------------------------------------------- accounts and lifecycle

  /** A Gmail user already signed in to the app. Call before `start`. */
  account(email: string, name?: string): Promise<{ id: number }> {
    return this.call("POST", "/__test/account", { email, name });
  }

  /** A Gmail user the app does not know yet (for Add account). */
  user(email: string, name?: string): Promise<null> {
    return this.call("POST", "/__test/user", { email, name });
  }

  /**
   * The request budget from the next `start` on: production's, or (the default)
   * one that never makes a request wait.
   */
  quota(production: boolean): Promise<null> {
    return this.call("POST", "/__test/quota", { production });
  }

  /** Launches the engine (or relaunches it after `stop`). Use `App.open` / `App.launch`. */
  start(): Promise<null> {
    return this.call("POST", "/__test/start");
  }

  /**
   * Stops the core. `graceful` quits as Cmd+Q does: the page closes an open
   * composer first. Otherwise it crashes. Use `App.quit` / `App.crash`.
   */
  stop(opts: { graceful: boolean }): Promise<null> {
    return this.call("POST", "/__test/stop", opts);
  }

  /** Waits until the core is idle (held requests excepted). Use `App.settle`. */
  idle(timeoutMs = 10_000): Promise<CoreIdle> {
    return this.call("GET", `/__test/idle?timeoutMs=${timeoutMs}`);
  }

  // ---------------------------------------------------------------- mail

  deliver(d: Delivery): Promise<Delivered> {
    return this.call("POST", "/__test/deliver", d);
  }

  /**
   * Many deliveries in one call, in order; an item's `after` joins an earlier
   * item's thread. Polls once at the end if any asks.
   */
  deliverMany(list: Delivery[]): Promise<Delivered[]> {
    return this.call("POST", "/__test/deliver-batch", list);
  }

  gmail(email: string): Promise<GmailState> {
    return this.call("GET", this.mailbox(email));
  }

  /** A label change made in Gmail web or by a filter. Does not poll unless asked. */
  modify(
    email: string,
    id: string,
    add: string[],
    remove: string[],
    opts: { poll?: boolean } = {},
  ): Promise<null> {
    return this.call("POST", this.mailbox(email, "/modify"), { id, add, remove, ...opts });
  }

  /** A message deleted forever in Gmail. Does not poll unless asked. */
  deleteMessage(email: string, id: string, opts: { poll?: boolean } = {}): Promise<null> {
    return this.call("POST", this.mailbox(email, "/delete"), { id, ...opts });
  }

  /** A whole thread deleted forever in Gmail. Does not poll unless asked. */
  deleteThread(email: string, threadId: string, opts: { poll?: boolean } = {}): Promise<null> {
    return this.call("POST", this.mailbox(email, "/delete-thread"), { id: threadId, ...opts });
  }

  /** A label created in Gmail. Returns its id. */
  label(email: string, name: string): Promise<{ id: string }> {
    return this.call("POST", this.mailbox(email, "/label"), { name });
  }

  /** A send-as alias added in Gmail settings. */
  sendAs(email: string, alias: string, displayName: string): Promise<null> {
    return this.call("POST", this.mailbox(email, "/send-as"), { email: alias, displayName });
  }

  /** A draft made in another client. Does not poll unless asked. */
  createDraft(email: string, d: DraftSpec, opts: { poll?: boolean } = {}): Promise<AddedDraft> {
    return this.call("POST", this.mailbox(email, "/drafts"), { ...d, ...opts });
  }

  /** A draft edited in another client: its message is replaced. */
  updateDraft(
    email: string,
    id: string,
    d: DraftSpec,
    opts: { poll?: boolean } = {},
  ): Promise<AddedDraft> {
    return this.call("POST", this.mailbox(email, `/drafts/${encodeURIComponent(id)}`), {
      ...d,
      ...opts,
    });
  }

  /** A draft deleted in another client. */
  deleteDraft(email: string, id: string, opts: { poll?: boolean } = {}): Promise<null> {
    return this.call("POST", this.mailbox(email, `/drafts/${encodeURIComponent(id)}/delete`), opts);
  }

  /** A draft sent from another client (a phone). */
  sendDraft(email: string, id: string, opts: { poll?: boolean } = {}): Promise<Delivered> {
    return this.call("POST", this.mailbox(email, `/drafts/${encodeURIComponent(id)}/send`), opts);
  }

  /** The account's access tokens expire: its next Gmail call gets a 401 and refreshes. */
  expireAccessTokens(email: string): Promise<null> {
    return this.call("POST", this.mailbox(email, "/expire-tokens"));
  }

  /** Gmail forgets the account's history: the next `history.list` answers 404. */
  expireHistory(email: string): Promise<null> {
    return this.call("POST", this.mailbox(email, "/expire-history"));
  }

  // ---------------------------------------------------------------- OAuth

  /** Google revokes (on) or restores (off) the account's refresh token. */
  invalidGrant(email: string, on: boolean): Promise<null> {
    return this.call("POST", "/__test/invalid-grant", { email, on });
  }

  /**
   * How the next browser sign-in goes (once): as `email` (default: the fake's
   * first user), abandoned (the tab is closed), or denied (`access_denied`).
   */
  nextSignIn(opts: { email?: string; abandon?: boolean; deny?: boolean }): Promise<null> {
    return this.call("POST", "/__test/sign-in", opts);
  }

  /** Stored refresh tokens, as the token file holds them, by account. */
  tokens(): Promise<TokenItem[]> {
    return this.call("GET", "/__test/tokens");
  }

  /** The token file's permission bits, such as "600", or null before the first save. */
  tokenFileMode(): Promise<string | null> {
    return this.call("GET", "/__test/tokens/mode");
  }

  /** Deletes an account's token from the file, as if it was never saved. */
  forgetToken(email: string): Promise<null> {
    return this.call("POST", "/__test/tokens/forget", { email });
  }

  /** The user's Gmail filters, oldest first. */
  filters(email: string): Promise<{ id: string; from: string; add: string[]; remove: string[] }[]> {
    return this.call("GET", `/__test/gmail/${encodeURIComponent(email)}/filters`);
  }

  /** Whether the user's grant includes Gmail filters, until the next sign-in decides. */
  settingsScope(email: string, granted: boolean): Promise<null> {
    return this.call("POST", "/__test/settings-scope", { email, granted });
  }

  /** Tokens revoked at Google, oldest first. */
  revocations(): Promise<{ token: string; account: string }[]> {
    return this.call("GET", "/__test/revocations");
  }

  // ---------------------------------------------------------------- faults

  /**
   * Makes matching Gmail requests fail. `on` is a method name or a full match;
   * `opts` adds to it (e.g. `{ times: 2 }`). Returns the fault's id.
   */
  async fault(
    on: ApiMethod | Match,
    fault: Outcome | { kind: "hold" } | { kind: "holdAnswer" },
    opts: Omit<Match, "api"> = {},
  ): Promise<number> {
    const match = typeof on === "string" ? { api: on } : on;
    const r = await this.call<{ id: number }>("POST", "/__test/fault", {
      ...match,
      ...opts,
      ...fault,
    });
    return r.id;
  }

  /**
   * Parks every matching request until released, so a test can look at the
   * app while the request is in flight. `settle()` does not wait for them.
   */
  async hold(on: ApiMethod | Match, opts: Omit<Match, "api"> = {}): Promise<Hold> {
    const id = await this.fault(on, { kind: "hold" }, opts);
    return {
      id,
      release: (outcome, wait = {}) => this.release({ id, min: wait.min ?? 1 }, outcome),
    };
  }

  /**
   * Does the work of every matching request at once, then parks its answer
   * until released: the app gets a reply that is stale by then. A release
   * outcome does not apply.
   */
  async holdAnswer(on: ApiMethod | Match, opts: Omit<Match, "api"> = {}): Promise<Hold> {
    const id = await this.fault(on, { kind: "holdAnswer" }, opts);
    return { id, release: (_, wait = {}) => this.release({ id, min: wait.min ?? 1 }) };
  }

  /**
   * Ends holds (one by id, those on one method, or all) and lets the requests
   * they parked go on: handled normally, or suffering `outcome`. With `min`,
   * first waits until they have parked that many. Returns how many.
   */
  async release(
    which: { id?: number; api?: ApiMethod; min?: number } = {},
    outcome?: Outcome,
  ): Promise<number> {
    const r = await this.call<{ released: number }>("POST", "/__test/release", {
      ...which,
      outcome,
    });
    return r.released;
  }

  /**
   * Removes every fault and hold, and retries what is backing off now, as a
   * network change would. Requests already held stay held until released.
   */
  clearFaults(): Promise<null> {
    return this.call("POST", "/__test/clear-faults");
  }

  /** The network goes away (every Gmail request fails to connect) or comes back. */
  offline(on: boolean): Promise<null> {
    return this.call("POST", "/__test/offline", { on });
  }

  /** The request log, in arrival order, optionally only one method's requests. */
  async requests(api?: ApiMethod, filter: { account?: string } = {}): Promise<LoggedRequest[]> {
    const all = await this.call<LoggedRequest[]>("GET", "/__test/requests");
    return all.filter(
      (r) =>
        (api === undefined || r.api === api) &&
        (filter.account === undefined || r.account === filter.account),
    );
  }

  /** A history poll runs now for every account. */
  poll(): Promise<null> {
    return this.call("POST", "/__test/poll");
  }

  // ---------------------------------------------------------------- time

  /** App time, Unix ms. */
  async now(): Promise<number> {
    const r = await this.call<{ now: number }>("GET", "/__test/clock");
    return r.now;
  }

  /**
   * Moves the core's clock forward and waits for the core to settle. Use
   * `App.advance`, which moves the browser's clock too.
   */
  advanceServerClock(ms: number, opts: { stepwise?: boolean } = {}): Promise<{ now: number }> {
    return this.call("POST", "/__test/clock", { advanceMs: ms, ...opts });
  }

  /** Moves the core's wall clock without firing timers (sleep). Use `App.sleep`. */
  jumpServerClock(ms: number): Promise<{ now: number }> {
    return this.call("POST", "/__test/clock", { jumpMs: ms });
  }

  // ---------------------------------------------------------------- platform

  /**
   * The window gains or loses focus, for the core. Use `App.focus`, which
   * also sets the page's `document.hasFocus()`. On focus, returns the
   * notification the focus counts as a click on, if any.
   */
  focus(
    focused: boolean,
  ): Promise<{ openThread: { account: string; thread: string | null } | null } | null> {
    return this.call("POST", "/__test/focus", { focused });
  }

  platform(): Promise<Recorded> {
    return this.call("GET", "/__test/platform");
  }

  /** Forgets recorded notifications, opened URLs and files, and focus requests. */
  clearRecorded(): Promise<null> {
    return this.call("POST", "/__test/clear-recorded");
  }

  /**
   * Fetches of the remote image host (`<url>/img/*`) and the hostile sink
   * (`<url>/sink/*`, where `App` routes `evil.test`).
   */
  hits(): Promise<{ method: string; path: string; body: string; at: number }[]> {
    return this.call("GET", "/__test/hits");
  }

  /**
   * Whether the SQLite files hold `text` anywhere. Only for what SPEC promises
   * with nothing on screen to show it (SEC-04): cases check the UI and Gmail.
   */
  async dbContains(text: string): Promise<boolean> {
    const q = new URLSearchParams({ text });
    const r = await this.call<{ found: boolean }>("GET", `/__test/db-contains?${q}`);
    return r.found;
  }
}
