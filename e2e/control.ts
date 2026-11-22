type Attachment = {
  filename: string;
  mimeType?: string;
  content: string;
  base64?: boolean;
  contentId?: string;
  inline?: boolean;
  servedInline?: boolean;
};

export type Delivery = {
  account: string;
  from: string;
  to?: string[];
  cc?: string[];
  subject: string;
  text?: string;
  html?: string;
  charset?: string;
  labels?: string[];
  threadId?: string;
  inReplyTo?: string;
  references?: string;
  listId?: string;
  headers?: [string, string][];
  attachments?: Attachment[];
  date?: number;
  after?: number;
  poll?: boolean;
};

export type Delivered = { id: string; threadId: string; messageId: string };

export type GmailMessage = {
  id: string;
  threadId: string;
  labels: string[];
  date: number;
  headers: Record<string, string>;
  raw: string;
};

export type GmailState = {
  messages: GmailMessage[];
  drafts: { id: string; message: GmailMessage }[];
  labels: { id: string; name: string }[];
  sendAs: { email: string; displayName: string; primary: boolean }[];
};

export type DraftSpec = {
  from?: string;
  to?: string[];
  cc?: string[];
  subject?: string;
  text?: string;
  html?: string;
  threadId?: string;
  inReplyTo?: string;
  references?: string;
  headers?: [string, string][];
  attachments?: Attachment[];
  date?: number;
};

export type AddedDraft = {
  id: string;
  messageId: string;
  threadId: string;
  rfc822MessageId: string;
};

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

export type Outcome =
  | { kind: "drop" }
  | { kind: "noResponse" }
  | { kind: "status"; status: number; reason?: string; retryAfterSecs?: number }
  | { kind: "statusAfterApply"; status: number; reason?: string }
  | { kind: "omitThreads"; threads: string[] };

type Match = {
  api?: ApiMethod;
  method?: string;
  path?: string;
  account?: string;
  query?: Record<string, string>;
  nth?: number;
  times?: number;
};

export type LoggedRequest = {
  seq: number;
  api: ApiMethod | "unknown";
  method: string;
  path: string;
  params: Record<string, string>;
  account: string | null;
  body: unknown;
  at: number;
  held: boolean;
  answer: number | "dropped" | "noResponse" | null;
  origin: string | null;
  userAgent: string | null;
};

export type Hold = {
  id: number;
  release: (outcome?: Outcome, opts?: { min?: number }) => Promise<number>;
};

type Notification = {
  title: string;
  body: string;
  account: string;
  thread: string | null;
};

type Recorded = {
  notifications: Notification[];
  badge: number | null;
  openedUrls: string[];
  openedFiles: string[];
  windowsShown: number;
  events: { event: string }[];
  exits: number;
};

type TokenItem = { account: string; token: string };

type CoreIdle = { rev: number; ack: number; signIns: number; held: number };

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
    // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- our own server; each method names its handler's shape
    return data as T;
  }

  private mailbox(email: string, rest = ""): string {
    return `/__test/gmail/${encodeURIComponent(email)}${rest}`;
  }

  account(email: string, name?: string): Promise<{ id: number }> {
    return this.call("POST", "/__test/account", { email, name });
  }

  user(email: string, name?: string): Promise<null> {
    return this.call("POST", "/__test/user", { email, name });
  }

  quota(production: boolean): Promise<null> {
    return this.call("POST", "/__test/quota", { production });
  }

  start(): Promise<null> {
    return this.call("POST", "/__test/start");
  }

  stop(opts: { graceful: boolean }): Promise<null> {
    return this.call("POST", "/__test/stop", opts);
  }

  idle(timeoutMs = 10_000): Promise<CoreIdle> {
    return this.call("GET", `/__test/idle?timeoutMs=${timeoutMs}`);
  }

  deliver(d: Delivery): Promise<Delivered> {
    return this.call("POST", "/__test/deliver", d);
  }

  deliverMany(list: Delivery[]): Promise<Delivered[]> {
    return this.call("POST", "/__test/deliver-batch", list);
  }

  gmail(email: string): Promise<GmailState> {
    return this.call("GET", this.mailbox(email));
  }

  async labelsOf(email: string, id: string): Promise<string[]> {
    const m = (await this.gmail(email)).messages.find((x) => x.id === id);
    if (!m) throw new Error(`no message ${id} in ${email}`);
    return m.labels;
  }

  modify(
    email: string,
    id: string,
    add: string[],
    remove: string[],
    opts: { poll?: boolean } = {},
  ): Promise<null> {
    return this.call("POST", this.mailbox(email, "/modify"), { id, add, remove, ...opts });
  }

  deleteMessage(email: string, id: string, opts: { poll?: boolean } = {}): Promise<null> {
    return this.call("POST", this.mailbox(email, "/delete"), { id, ...opts });
  }

  deleteThread(email: string, threadId: string, opts: { poll?: boolean } = {}): Promise<null> {
    return this.call("POST", this.mailbox(email, "/delete-thread"), { id: threadId, ...opts });
  }

  label(email: string, name: string): Promise<{ id: string }> {
    return this.call("POST", this.mailbox(email, "/label"), { name });
  }

  sendAs(email: string, alias: string, displayName: string): Promise<null> {
    return this.call("POST", this.mailbox(email, "/send-as"), { email: alias, displayName });
  }

  createDraft(email: string, d: DraftSpec, opts: { poll?: boolean } = {}): Promise<AddedDraft> {
    return this.call("POST", this.mailbox(email, "/drafts"), { ...d, ...opts });
  }

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

  deleteDraft(email: string, id: string, opts: { poll?: boolean } = {}): Promise<null> {
    return this.call("POST", this.mailbox(email, `/drafts/${encodeURIComponent(id)}/delete`), opts);
  }

  sendDraft(email: string, id: string, opts: { poll?: boolean } = {}): Promise<Delivered> {
    return this.call("POST", this.mailbox(email, `/drafts/${encodeURIComponent(id)}/send`), opts);
  }

  expireAccessTokens(email: string): Promise<null> {
    return this.call("POST", this.mailbox(email, "/expire-tokens"));
  }

  expireHistory(email: string): Promise<null> {
    return this.call("POST", this.mailbox(email, "/expire-history"));
  }

  invalidGrant(email: string, on: boolean): Promise<null> {
    return this.call("POST", "/__test/invalid-grant", { email, on });
  }

  nextSignIn(opts: { email?: string; abandon?: boolean; deny?: boolean }): Promise<null> {
    return this.call("POST", "/__test/sign-in", opts);
  }

  tokens(): Promise<TokenItem[]> {
    return this.call("GET", "/__test/tokens");
  }

  tokenFileMode(): Promise<string | null> {
    return this.call("GET", "/__test/tokens/mode");
  }

  forgetToken(email: string): Promise<null> {
    return this.call("POST", "/__test/tokens/forget", { email });
  }

  filters(email: string): Promise<{ id: string; from: string; add: string[]; remove: string[] }[]> {
    return this.call("GET", `/__test/gmail/${encodeURIComponent(email)}/filters`);
  }

  settingsScope(email: string, granted: boolean): Promise<null> {
    return this.call("POST", "/__test/settings-scope", { email, granted });
  }

  revocations(): Promise<{ token: string; account: string }[]> {
    return this.call("GET", "/__test/revocations");
  }

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

  async hold(on: ApiMethod | Match, opts: Omit<Match, "api"> = {}): Promise<Hold> {
    const id = await this.fault(on, { kind: "hold" }, opts);
    return {
      id,
      release: (outcome, wait = {}) => this.release({ id, min: wait.min ?? 1 }, outcome),
    };
  }

  async holdAnswer(on: ApiMethod | Match, opts: Omit<Match, "api"> = {}): Promise<Hold> {
    const id = await this.fault(on, { kind: "holdAnswer" }, opts);
    return { id, release: (_, wait = {}) => this.release({ id, min: wait.min ?? 1 }) };
  }

  async release(which: { id: number; min?: number }, outcome?: Outcome): Promise<number> {
    const r = await this.call<{ released: number }>("POST", "/__test/release", {
      ...which,
      outcome,
    });
    return r.released;
  }

  clearFaults(): Promise<null> {
    return this.call("POST", "/__test/clear-faults");
  }

  offline(on: boolean, opts: { probe?: boolean } = {}): Promise<null> {
    return this.call("POST", "/__test/offline", { on, ...opts });
  }

  async requests(api?: ApiMethod, filter: { account?: string } = {}): Promise<LoggedRequest[]> {
    const all = await this.call<LoggedRequest[]>("GET", "/__test/requests");
    return all.filter(
      (r) =>
        (api === undefined || r.api === api) &&
        (filter.account === undefined || r.account === filter.account),
    );
  }

  poll(): Promise<null> {
    return this.call("POST", "/__test/poll");
  }

  async now(): Promise<number> {
    const r = await this.call<{ now: number }>("GET", "/__test/clock");
    return r.now;
  }

  advanceServerClock(ms: number, opts: { stepwise?: boolean } = {}): Promise<{ now: number }> {
    return this.call("POST", "/__test/clock", { advanceMs: ms, ...opts });
  }

  jumpServerClock(ms: number): Promise<{ now: number }> {
    return this.call("POST", "/__test/clock", { jumpMs: ms });
  }

  focus(
    focused: boolean,
  ): Promise<{ openThread: { account: string; thread: string | null } | null } | null> {
    return this.call("POST", "/__test/focus", { focused });
  }

  platform(): Promise<Recorded> {
    return this.call("GET", "/__test/platform");
  }

  clearRecorded(): Promise<null> {
    return this.call("POST", "/__test/clear-recorded");
  }

  hits(): Promise<{ method: string; path: string; body: string; at: number }[]> {
    return this.call("GET", "/__test/hits");
  }

  async dbContains(text: string): Promise<boolean> {
    const q = new URLSearchParams({ text });
    const r = await this.call<{ found: boolean }>("GET", `/__test/db-contains?${q}`);
    return r.found;
  }

  draftFolders(): Promise<string[]> {
    return this.call("GET", "/__test/draft-folders");
  }

  async leaveDraftFolder(name: string): Promise<void> {
    await this.call("POST", "/__test/draft-folders", { name });
  }

  async blockDraftFolders(): Promise<void> {
    await this.call("POST", "/__test/draft-folders/block");
  }
}
