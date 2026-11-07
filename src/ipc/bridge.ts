// The UI's only door to Rust: numbered actions sent one invoke at a time,
// subscriptions, a few plain commands, and app events.

import { Channel, invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { subForKey, type MirrorState } from "../store/mirror-core";
import { useMirror } from "../store/mirror";
import type {
  Action,
  AppEvent,
  ChannelMessage,
  Command,
  CommandReply,
  LaunchInfo,
  NumberedAction,
  Rejection,
  SubKey,
} from "./types";

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
const SIGN_INS = new Set<CommandName>(["add_account", "sign_in_again"]);

/**
 * Rust read the command and answered with an error, so sending it again
 * would get the same answer. A command that got no answer throws an `Error`.
 */
class Refused extends Error {}

/** A command's name, as Rust defines it (`ub_ipc::Command`). */
type CommandName = Command["command"];
/** A command's arguments, named as Rust names them. */
type ArgsOf<K extends CommandName> = Extract<Command, { command: K }>["args"];
/** What a command replies. */
type ReplyOf<K extends CommandName> = Extract<CommandReply, { command: K }>["reply"];

/**
 * Runs a command in Rust. Its name, arguments, and reply are typed from
 * `ub_ipc::Command`. In Tauri, every command but `connect` goes through the
 * one `dispatch` command, which runs the web bridge's code.
 */
async function invoke<K extends CommandName>(command: K, args: ArgsOf<K>): Promise<ReplyOf<K>> {
  if (inTauri) return tauriInvoke<ReplyOf<K>>("dispatch", { command, args });
  const kind = SIGN_INS.has(command) ? "signIns" : "invokes";
  pending[kind]++;
  try {
    const r = await fetch(`${BRIDGE}/invoke/${command}`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(args),
    });
    const body: unknown = await r.json();
    // A refused command answers 400 with its error string; 503 means no core answered.
    const text = typeof body === "string" ? body : JSON.stringify(body);
    if (r.status === 400) throw new Refused(text);
    if (!r.ok) throw new Error(text);
    // The debug bridge answers with JSON Rust serialized as this command's reply.
    // oxlint-disable-next-line typescript/no-unsafe-type-assertion
    return body as ReplyOf<K>;
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
// Until `connect` answers; nothing is attached before then, and Rust checks the limit too.
export let launch: LaunchInfo = { lastAction: 0, debug: false, maxAttachmentBytes: Infinity };

/** The wait before resending a failed submit; doubles while submits keep failing. */
let retryMs = 0;

/** What the user is told about an action Rust could not read. */
const UNREADABLE = "Couldn't do that because of a bug in Ubermensch";

/** Numbers the UI dropped unsent: Rust never commits them. */
const dropped = new Set<number>();

async function flush(): Promise<void> {
  if (inFlight || buffer.length === 0) return;
  inFlight = true;
  const actions = buffer;
  buffer = [];
  const ns = actions.map((a) => a.n);
  try {
    await invoke("submit", { actions });
    retryMs = 0;
  } catch (e) {
    // In Tauri a failed invoke is Rust's answer; on the web bridge, a 400.
    if (inTauri || e instanceof Refused) {
      // Rust can't read this batch; resent, it would hold up every later action.
      console.error(`submit of actions ${ns.join(", ")} refused; dropping them`, e);
      refuseUnsent(actions);
      return;
    }
    // Resending is safe: Rust drops numbers at or below its ack.
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
  // Every number up to `last` is acked, or was dropped unsent.
  const committed = (ack: number) => {
    for (let n = ack + 1; n <= last; n++) if (!dropped.has(n)) return false;
    return true;
  };
  return new Promise((resolve) => {
    if (committed(useMirror.getState().ack)) return resolve();
    const done = () => {
      stop();
      clearTimeout(timer);
      resolve();
    };
    const stop = useMirror.subscribe((s) => {
      if (committed(s.ack)) done();
    });
    const timer = setTimeout(() => {
      console.warn(`actions up to ${last} not committed after ${timeoutMs} ms; going on`);
      done();
    }, timeoutMs);
  });
}

/** Web mode: the UI's pending work, and the last channel message it applied. */
export const pendingWork = () => ({ ...pending, buffered: buffer.length });

/** What Rust did with an action, as the channel message that settles it says. */
export type Outcome = {
  /** Why Rust refused it, or null: Rust applied it. */
  refusal: Rejection | null;
  /** The mirror just before that message: the state Rust applied the action to. */
  before: MirrorState;
};

/** Callers waiting for their action's outcome, by action number. */
const settling = new Map<number, (outcome: Outcome) => void>();

/** Shows a refusal no caller waits for, with the action when this page sent it. */
type ShowRefusal = (refusal: Rejection, action: Action | null) => void;
let showRefusal: ShowRefusal = (r) => console.warn(`action ${r.n} refused: ${r.message}`);

/** Sets how refusals no caller waits for are shown (main.tsx). */
export function onRefusal(show: ShowRefusal): void {
  showRefusal = show;
}

/**
 * Sends an action. The mirror applies its effect in the same frame. With
 * `settled`, the caller hears Rust's outcome once a channel message settles
 * the action, and shows a refusal itself; without, a refusal shows as
 * `onRefusal` says.
 */
export function act(action: Action, settled?: (outcome: Outcome) => void): number {
  const n = nextN++;
  if (settled) settling.set(n, settled);
  useMirror.getState().pend({ n, action });
  buffer.push({ n, action });
  void flush();
  return n;
}

/**
 * Applies a channel message to the mirror, then settles the actions it
 * decides (SPEC UI state, Actions): those it lists are refused, and the rest
 * its `ack` covers were applied.
 */
function receive(msg: ChannelMessage): void {
  const before = useMirror.getState();
  before.receive(msg);
  for (const refusal of msg.rejected) {
    const settled = settling.get(refusal.n);
    settling.delete(refusal.n);
    if (settled) settled({ refusal, before });
    else showRefusal(refusal, before.pending.find((p) => p.n === refusal.n)?.action ?? null);
  }
  for (const [n, settled] of settling) {
    if (n > msg.ack) continue;
    settling.delete(n);
    settled({ refusal: null, before });
  }
}

/**
 * Actions Rust could not read as a batch, dropped unsent: the mirror falls
 * back to Rust's state, and each is refused as Rust refuses one it can't read.
 */
function refuseUnsent(actions: NumberedAction[]): void {
  const before = useMirror.getState();
  before.dropPending(actions.map((a) => a.n));
  for (const { n, action } of actions) {
    dropped.add(n);
    const refusal: Rejection = { n, message: UNREADABLE, split: null };
    const settled = settling.get(n);
    settling.delete(n);
    if (settled) settled({ refusal, before });
    else showRefusal(refusal, action);
  }
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

/** An app event's name, as Rust defines it (`ub_ipc::AppEvent`). */
type EventName = AppEvent["event"];
/** An app event's payload; null for one that has none. */
type PayloadOf<K extends EventName> =
  Extract<AppEvent, { event: K }> extends { payload: infer P } ? P : null;

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
export function onEvent<K extends EventName>(
  name: K,
  handler: (payload: PayloadOf<K>) => void,
): void {
  if (inTauri) {
    void listen<PayloadOf<K>>(name, (e) => handler(e.payload));
    return;
  }
  // The payload is JSON Rust serialized for this event name.
  // oxlint-disable-next-line typescript/no-unsafe-type-assertion
  const untyped = (payload: unknown) => handler(payload as PayloadOf<K>);
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
    channel.onmessage = receive;
    // `connect` is Tauri's own command: it takes the channel.
    launch = await tauriInvoke<LaunchInfo>("connect", { channel });
  } else {
    const source = new EventSource(`${BRIDGE}/events`);
    events = source;
    source.addEventListener("message", (e: MessageEvent<string>) => {
      // Channel messages are serialized by Rust from the ts-rs types.
      // oxlint-disable-next-line typescript/no-unsafe-type-assertion
      const msg = JSON.parse(e.data) as ChannelMessage;
      receive(msg);
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
    launch = await invoke("connect", {});
  }
  nextN = launch.lastAction + 1;
  // Close every subscription of a previous page (e.g. after a webview crash).
  act({ type: "reset" });
  return launch;
}

// ------------------------------------------------------------ commands

export const fetchRows = (drafts: boolean, keys: string[]) =>
  invoke("fetch_rows", { drafts, keys });
export const getBodies = (ids: number[]) => invoke("get_bodies", { ids });
export const autocomplete = (account: number, query: string) =>
  invoke("autocomplete", { account, query });
export const addAccount = () => invoke("add_account", {});
export const signInAgain = (account: number) => invoke("sign_in_again", { account });
export const openAttachment = (attachment: number) => invoke("open_attachment", { attachment });
export const quitNow = () => invoke("quit_now", {});
/** Web mode: a link from a message, opened by the core as the opener plugin would. */
export const openUrlInCore = (url: string) => invoke("open_url", { url });
export const unsubscribeFrom = (account: number, thread: number) =>
  invoke("unsubscribe", { account, thread });
export const senderThreads = (account: number, thread: number) =>
  invoke("sender_threads", { account, thread });
export const blockSender = (account: number, thread: number, domain: boolean) =>
  invoke("block", { account, thread, domain });
export const blockedSenders = (account: number) => invoke("blocked_senders", { account });
export const unblockSender = (account: number, id: string) => invoke("unblock", { account, id });
export const perfReport = (action: string, ms: number, budget: number) =>
  invoke("perf_report", { action, ms, budget });
export const snoozeOptions = (text: string) => invoke("snooze_options", { text });
export const addAttachment = (draft: string, filename: string, mimeType: string, data: string) =>
  invoke("add_attachment", { draft, filename, mimeType, data });
