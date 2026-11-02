// A typed client for the E2E server's `/__test` control API
// (src-tauri/src/engine/testserver.rs).

export type Attachment = {
  filename: string;
  mimeType?: string;
  /** Text content, or base64 with `base64: true`. */
  content: string;
  base64?: boolean;
  contentId?: string;
};

export type Delivery = {
  /** The mailbox the message lands in. */
  account: string;
  from: string;
  to?: string[];
  cc?: string[];
  subject: string;
  text?: string;
  html?: string;
  /** Gmail label ids. Default: INBOX, UNREAD. */
  labels?: string[];
  /** Joins this Gmail thread. */
  threadId?: string;
  inReplyTo?: string;
  listId?: string;
  headers?: [string, string][];
  attachments?: Attachment[];
  /** Unix ms. Default: app time now. */
  date?: number;
  /** Poll at once so the app sees it without waiting. Default true. */
  poll?: boolean;
};

export type Delivered = { id: string; threadId: string; messageId: string };

export type GmailMessage = {
  id: string;
  threadId: string;
  labels: string[];
  date: number;
  snippet: string;
  /** Lower-cased header names. */
  headers: Record<string, string>;
  /** The raw RFC 822 message. */
  raw: string;
};

export type GmailState = {
  messages: GmailMessage[];
  drafts: { id: string; message: GmailMessage }[];
  labels: { id: string; name: string }[];
  historyId: number;
};

export type Fault =
  | { kind: "drop" }
  | { kind: "noResponse" }
  | { kind: "status"; status: number; reason?: string; retryAfterSecs?: number }
  | { kind: "delay"; delayMs: number };

export type FaultMatch = {
  /** HTTP method; any when omitted. */
  method?: string;
  /** Glob on the Gmail request path, e.g. `*\/messages/batchModify`. */
  path: string;
  /** Only the nth matching request (1-based, counted from injection). */
  nth?: number;
  /** Only the first n matching requests. */
  times?: number;
};

export type Recorded = {
  notifications: { title: string; body: string }[];
  badge: number | null;
  openedUrls: string[];
  openedFiles: string[];
  focusRequests: number;
};

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

  /** A Gmail account already signed in to the app. Call before `start`. */
  account(email: string, name?: string): Promise<{ id: number }> {
    return this.call("POST", "/__test/account", { email, name });
  }

  /** A Gmail user the app does not know yet (for Add account). */
  user(email: string, name?: string): Promise<null> {
    return this.call("POST", "/__test/user", { email, name });
  }

  /** Launches the engine: initial sync of the seeded accounts begins. */
  start(): Promise<null> {
    return this.call("POST", "/__test/start");
  }

  deliver(d: Delivery): Promise<Delivered> {
    return this.call("POST", "/__test/deliver", d);
  }

  gmail(email: string): Promise<GmailState> {
    return this.call("GET", `/__test/gmail/${encodeURIComponent(email)}`);
  }

  /** A label change made in Gmail web or by a filter. */
  modify(email: string, id: string, add: string[], remove: string[]): Promise<null> {
    return this.call("POST", `/__test/gmail/${encodeURIComponent(email)}/modify`, {
      id,
      add,
      remove,
    });
  }

  deleteMessage(email: string, id: string): Promise<null> {
    return this.call("POST", `/__test/gmail/${encodeURIComponent(email)}/delete`, { id });
  }

  /** A label created in Gmail. Returns its id. */
  label(email: string, name: string): Promise<{ id: string }> {
    return this.call("POST", `/__test/gmail/${encodeURIComponent(email)}/label`, { name });
  }

  sendAs(email: string, alias: string, displayName: string): Promise<null> {
    return this.call("POST", `/__test/gmail/${encodeURIComponent(email)}/send-as`, {
      email: alias,
      displayName,
    });
  }

  /** Google revokes (on) or restores (off) the account's refresh token. */
  invalidGrant(email: string, on: boolean): Promise<null> {
    return this.call("POST", "/__test/invalid-grant", { email, on });
  }

  /** How the next browser sign-in goes. */
  nextSignIn(opts: { email?: string; abandon?: boolean }): Promise<null> {
    return this.call("POST", "/__test/sign-in", opts);
  }

  fault(match: FaultMatch, fault: Fault): Promise<null> {
    return this.call("POST", "/__test/fault", { ...match, ...fault });
  }

  clearFaults(): Promise<null> {
    return this.call("POST", "/__test/clear-faults");
  }

  offline(on: boolean): Promise<null> {
    return this.call("POST", "/__test/offline", { on });
  }

  /** How many Gmail requests matched (method any when omitted). */
  async requestCount(path: string, method?: string): Promise<number> {
    const q = new URLSearchParams({ path, ...(method ? { method } : {}) });
    const r = await this.call<{ count: number }>("GET", `/__test/requests?${q}`);
    return r.count;
  }

  poll(): Promise<null> {
    return this.call("POST", "/__test/poll");
  }

  /** Moves app time forward on the server side. Use `App.advance` in tests. */
  advanceServerClock(ms: number): Promise<{ now: number }> {
    return this.call("POST", "/__test/clock", { advanceMs: ms });
  }

  /** The window gains or loses focus. */
  focus(focused: boolean): Promise<{ openThread?: { account: number; thread: number } } | null> {
    return this.call("POST", "/__test/focus", { focused });
  }

  platform(): Promise<Recorded> {
    return this.call("GET", "/__test/platform");
  }
}
