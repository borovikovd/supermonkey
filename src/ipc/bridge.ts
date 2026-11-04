// The UI's only door to Rust: numbered actions sent one invoke at a time,
// subscriptions, a few plain commands, and app events.

import { Channel, invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { subForKey } from "../store/mirror-core";
import { useMirror } from "../store/mirror";
import type {
  AccountId,
  Action,
  ChannelMessage,
  ContactSuggestion,
  DraftAttachment,
  LaunchInfo,
  ListRow,
  MessageBody,
  NumberedAction,
  SnoozeChoice,
  SubKey,
  Unsubscribed,
} from "./bridge-types";

/** Inside the Tauri webview; otherwise the debug web bridge on 127.0.0.1:1430. */
export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
// Outside Tauri: the dev server talks to the debug app's bridge; a built UI is
// served by the E2E server, which mounts the bridge on its own origin.
const BRIDGE = import.meta.env.DEV ? "http://127.0.0.1:1430" : "";

/**
 * Web mode only: what the UI still owes the core, for the E2E harness's
 * `settle()`. Sign-ins are counted apart because they can wait on a person.
 */
const pending = { invokes: 0, signIns: 0, timers: 0, rev: 0, ack: 0 };
const SIGN_INS = new Set(["add_account", "sign_in_again"]);

async function invoke<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> {
  if (inTauri) return tauriInvoke<T>(cmd, args);
  const kind = SIGN_INS.has(cmd) ? "signIns" : "invokes";
  pending[kind]++;
  try {
    const r = await fetch(`${BRIDGE}/invoke/${cmd}`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(args),
    });
    // The debug bridge answers with JSON Rust serialized from the same ts-rs types.
    // oxlint-disable-next-line typescript/no-unsafe-type-assertion
    const body = (await r.json()) as T;
    if (!r.ok) throw new Error(String(body));
    return body;
  } finally {
    pending[kind]--;
  }
}

/**
 * `setTimeout` for a callback that sends work to the core (a debounced save,
 * a prefetch). Returns a cancel function. Web mode counts it as pending.
 */
export function later(fn: () => void, ms: number): () => void {
  let live = true;
  pending.timers++;
  const end = () => {
    if (live) pending.timers--;
    live = false;
  };
  const t = setTimeout(() => {
    end();
    fn();
  }, ms);
  return () => {
    clearTimeout(t);
    end();
  };
}

let nextN = 1;
let buffer: NumberedAction[] = [];
let inFlight = false;
export let launch: LaunchInfo = { lastAction: 0, debug: false };

/** The wait before resending a failed submit; doubles while submits keep failing. */
let retryMs = 0;

async function flush(): Promise<void> {
  if (inFlight || buffer.length === 0) return;
  inFlight = true;
  const actions = buffer;
  buffer = [];
  try {
    await invoke("submit", { actions });
    retryMs = 0;
  } catch (e) {
    // Resending is safe: Rust drops numbers at or below its ack.
    const ns = actions.map((a) => a.n);
    console.warn(`submit of actions ${ns.join(", ")} failed; sending again`, e);
    buffer = [...actions, ...buffer];
    retryMs = Math.min(Math.max(retryMs * 2, 100), 5000);
    await new Promise((resolve) => setTimeout(resolve, retryMs));
  } finally {
    inFlight = false;
    void flush();
  }
}

/**
 * Resolves once Rust has committed every action sent so far, or after
 * `timeoutMs` if it does not answer (logged).
 */
export function whenCommitted(timeoutMs: number): Promise<void> {
  const last = nextN - 1;
  return new Promise((resolve) => {
    if (useMirror.getState().ack >= last) return resolve();
    const done = () => {
      stop();
      clearTimeout(timer);
      resolve();
    };
    const stop = useMirror.subscribe((s) => {
      if (s.ack >= last) done();
    });
    const timer = setTimeout(() => {
      console.warn(`actions up to ${last} not committed after ${timeoutMs} ms; going on`);
      done();
    }, timeoutMs);
  });
}

/** Web mode: the UI's pending work, and the last channel message it applied. */
export const pendingWork = () => ({ ...pending, buffered: buffer.length });

/** Sends an action. The mirror applies its effect in the same frame. */
export function act(action: Action): number {
  const n = nextN++;
  useMirror.getState().pend({ n, action });
  buffer.push({ n, action });
  void flush();
  return n;
}

// ------------------------------------------------------------ subscriptions

const byKey = new Map<string, { id: string; refs: number }>();
let subSeq = 0;
/**
 * Makes this page's subscription ids unlike any earlier page's, so a result
 * Rust queued for an old page's subscription can't land on a new one.
 */
const pageTag = Array.from(crypto.getRandomValues(new Uint8Array(4)), (b) =>
  b.toString(16).padStart(2, "0"),
).join("");

export const keyString = (key: SubKey) => JSON.stringify(key);

/** Subscribes (ref-counted by key) and returns the subscription id. */
export function subscribe(key: SubKey): string {
  const k = keyString(key);
  const have = byKey.get(k);
  if (have) {
    have.refs++;
    return have.id;
  }
  const id = `s${++subSeq}-${pageTag}`;
  byKey.set(k, { id, refs: 1 });
  useMirror.getState().addSub(id, subForKey(key));
  act({ type: "subscribe", sub: id, key });
  return id;
}

export function unsubscribe(key: SubKey): void {
  const k = keyString(key);
  const have = byKey.get(k);
  if (!have) return;
  if (--have.refs > 0) return;
  byKey.delete(k);
  act({ type: "unsubscribe", sub: have.id });
  useMirror.getState().dropSub(have.id);
}

/** Forgets a composer subscription that Rust closed together with an action. */
export function forgetSub(key: SubKey): void {
  const k = keyString(key);
  const have = byKey.get(k);
  if (!have) return;
  byKey.delete(k);
  useMirror.getState().dropSub(have.id);
}

export function subIdFor(key: SubKey): string | undefined {
  return byKey.get(keyString(key))?.id;
}

// ------------------------------------------------------------ app events

/** App events Rust sends the page, and their payloads (see lib.rs and the E2E server). */
export type AppEvents = {
  /** A notification was clicked: that account's Inbox, the thread highlighted. */
  "notification-open": { account: number; thread: number | null };
  /** Cmd+Q: close an open composer, then call `quit_now`. */
  "quit-requested": null;
};

type EventHandler = (payload: unknown) => void;
const handlers = new Map<string, EventHandler[]>();
let events: EventSource | null = null;

function listenWeb(source: EventSource, name: string) {
  source.addEventListener(name, (e: MessageEvent<string>) => {
    const payload: unknown = JSON.parse(e.data);
    for (const h of handlers.get(name) ?? []) h(payload);
  });
}

/** An app event from Rust: Tauri's `emit`, or a named event on the web bridge. */
export function onEvent<K extends keyof AppEvents>(
  name: K,
  handler: (payload: AppEvents[K]) => void,
): void {
  if (inTauri) {
    void listen<AppEvents[K]>(name, (e) => handler(e.payload));
    return;
  }
  // The payload is JSON Rust serialized for this event name.
  // oxlint-disable-next-line typescript/no-unsafe-type-assertion
  const untyped = (payload: unknown) => handler(payload as AppEvents[K]);
  const have = handlers.get(name);
  if (have) have.push(untyped);
  else {
    handlers.set(name, [untyped]);
    if (events) listenWeb(events, name);
  }
}

// ------------------------------------------------------------ startup

export async function connect(): Promise<LaunchInfo> {
  if (inTauri) {
    const channel = new Channel<ChannelMessage>();
    // Tauri's Channel is not an EventTarget; `onmessage` is its only API.
    // oxlint-disable-next-line unicorn/prefer-add-event-listener
    channel.onmessage = (msg) => useMirror.getState().receive(msg);
    launch = await invoke<LaunchInfo>("connect", { channel });
  } else {
    const source = new EventSource(`${BRIDGE}/events`);
    events = source;
    source.addEventListener("message", (e: MessageEvent<string>) => {
      // Channel messages are serialized by Rust from the ts-rs types.
      // oxlint-disable-next-line typescript/no-unsafe-type-assertion
      const msg = JSON.parse(e.data) as ChannelMessage;
      useMirror.getState().receive(msg);
      pending.rev = msg.rev;
      pending.ack = msg.ack;
    });
    for (const name of handlers.keys()) listenWeb(source, name);
    await new Promise<void>((resolve) => {
      source.addEventListener("open", () => resolve(), { once: true });
    });
    // The stream reconnected: messages sent meanwhile are lost, and the core
    // may have restarted. Start over, as a reload resyncs everything.
    source.addEventListener("open", () => location.reload());
    launch = await invoke<LaunchInfo>("connect");
  }
  nextN = launch.lastAction + 1;
  // Close every subscription of a previous page (e.g. after a webview crash).
  act({ type: "reset" });
  return launch;
}

// ------------------------------------------------------------ commands

export const fetchRows = (drafts: boolean, keys: string[]) =>
  invoke<ListRow[]>("fetch_rows", { drafts, keys });
export const getBodies = (ids: number[]) => invoke<MessageBody[]>("get_bodies", { ids });
export const autocomplete = (account: AccountId, query: string) =>
  invoke<ContactSuggestion[]>("autocomplete", { account, query });
export const addAccount = () => invoke<number>("add_account");
export const signInAgain = (account: AccountId) => invoke<void>("sign_in_again", { account });
export const openAttachment = (attachment: number) =>
  invoke<void>("open_attachment", { attachment });
export const quitNow = () => invoke<void>("quit_now");
/** Web mode: a link from a message, opened by the core as the opener plugin would. */
export const openUrlInCore = (url: string) => invoke<void>("open_url", { url });
export const unsubscribeFrom = (account: AccountId, thread: number) =>
  invoke<Unsubscribed>("unsubscribe", { account, thread });
export const senderThreads = (account: AccountId, thread: number) =>
  invoke<number[]>("sender_threads", { account, thread });
export const perfReport = (action: string, ms: number, budget: number) =>
  invoke<void>("perf_report", { action, ms, budget });
export const snoozeOptions = (text: string) => invoke<SnoozeChoice[]>("snooze_options", { text });
export const addAttachment = (draft: string, filename: string, mimeType: string, data: string) =>
  invoke<DraftAttachment>("add_attachment", { draft, filename, mimeType, data });
