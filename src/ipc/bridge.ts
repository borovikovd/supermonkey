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

export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
const BRIDGE = import.meta.env.DEV ? "http://127.0.0.1:1430" : "";

const pending = { invokes: 0, signIns: 0, timers: 0, rev: 0, ack: 0 };
const SIGN_INS = new Set<CommandName>(["add_account", "sign_in_again"]);

class Refused extends Error {}

type CommandName = Command["command"];
type ArgsOf<K extends CommandName> = Extract<Command, { command: K }>["args"];
type ReplyOf<K extends CommandName> = Extract<CommandReply, { command: K }>["reply"];

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
    const text = typeof body === "string" ? body : JSON.stringify(body);
    if (r.status === 400) throw new Refused(text);
    if (!r.ok) throw new Error(text);
    // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- the debug bridge answers with this command's reply, serialized by Rust
    return body as ReplyOf<K>;
  } finally {
    pending[kind]--;
  }
}

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
export let launch: LaunchInfo = { lastAction: 0, debug: false, maxAttachmentBytes: Infinity };

let retryMs = 0;

const UNREADABLE = "Couldn't do that because of a bug in Supermonkey";

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
    if (inTauri || e instanceof Refused) {
      console.error(`submit of actions ${ns.join(", ")} refused; dropping them`, e);
      refuseUnsent(actions);
      return;
    }
    console.warn(`submit of actions ${ns.join(", ")} failed; sending again`, e);
    buffer = [...actions, ...buffer];
    retryMs = Math.min(Math.max(retryMs * 2, 100), 5000);
    await new Promise((resolve) => setTimeout(resolve, retryMs));
  } finally {
    inFlight = false;
    void flush();
  }
}

export function whenCommitted(timeoutMs: number): Promise<void> {
  const last = nextN - 1;
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

export const pendingWork = () => ({ ...pending, buffered: buffer.length });

export type Outcome = {
  refusal: Rejection | null;
  before: MirrorState;
};

const settling = new Map<number, (outcome: Outcome) => void>();

type ShowRefusal = (refusal: Rejection, action: Action | null) => void;
let showRefusal: ShowRefusal = (r) => console.warn(`action ${r.n} refused: ${r.message}`);

export function onRefusal(show: ShowRefusal): void {
  showRefusal = show;
}

export function act(action: Action, settled?: (outcome: Outcome) => void): number {
  const n = nextN++;
  if (settled) settling.set(n, settled);
  useMirror.getState().pend({ n, action });
  buffer.push({ n, action });
  void flush();
  return n;
}

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

const byKey = new Map<string, { id: string; refs: number }>();
let subSeq = 0;
const pageTag = Array.from(crypto.getRandomValues(new Uint8Array(4)), (b) =>
  b.toString(16).padStart(2, "0"),
).join("");

export const keyString = (key: SubKey) => JSON.stringify(key);

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

export function forgetSub(key: SubKey): void {
  const k = keyString(key);
  const have = byKey.get(k);
  if (!have) return;
  if (--have.refs > 0) return;
  byKey.delete(k);
  useMirror.getState().dropSub(have.id);
}

export function resubscribe(key: SubKey): void {
  const k = keyString(key);
  const have = byKey.get(k);
  const id = `s${++subSeq}-${pageTag}`;
  byKey.set(k, { id, refs: have?.refs ?? 1 });
  useMirror.getState().addSub(id, subForKey(key));
  act({ type: "subscribe", sub: id, key });
  if (!have) return;
  act({ type: "unsubscribe", sub: have.id });
  useMirror.getState().dropSub(have.id);
}

export function subIdFor(key: SubKey): string | undefined {
  return byKey.get(keyString(key))?.id;
}

type EventName = AppEvent["event"];
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

export function onEvent<K extends EventName>(
  name: K,
  handler: (payload: PayloadOf<K>) => void,
): void {
  if (inTauri) {
    void listen<PayloadOf<K>>(name, (e) => handler(e.payload));
    return;
  }
  // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- the payload is this event's type, serialized by Rust
  const untyped = (payload: unknown) => handler(payload as PayloadOf<K>);
  const have = handlers.get(name);
  if (have) have.push(untyped);
  else {
    handlers.set(name, [untyped]);
    if (events) listenWeb(events, name);
  }
}

export async function connect(): Promise<LaunchInfo> {
  if (inTauri) {
    const channel = new Channel<ChannelMessage>();
    // oxlint-disable-next-line unicorn/prefer-add-event-listener -- Tauri's Channel has only onmessage
    channel.onmessage = receive;
    launch = await tauriInvoke<LaunchInfo>("connect", { channel });
  } else {
    const source = new EventSource(`${BRIDGE}/events`);
    events = source;
    source.addEventListener("message", (e: MessageEvent<string>) => {
      // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- channel messages are serialized by Rust from the ts-rs types
      const msg = JSON.parse(e.data) as ChannelMessage;
      receive(msg);
      pending.rev = msg.rev;
      pending.ack = msg.ack;
    });
    for (const name of handlers.keys()) listenWeb(source, name);
    await new Promise<void>((resolve) => {
      source.addEventListener("open", () => resolve(), { once: true });
    });
    source.addEventListener("open", () => location.reload());
    launch = await invoke("connect", {});
  }
  nextN = launch.lastAction + 1;
  act({ type: "reset" });
  return launch;
}

export const fetchRows = (drafts: boolean, keys: string[]) =>
  invoke("fetch_rows", { drafts, keys });
export const getBodies = (ids: number[]) => invoke("get_bodies", { ids });
export const autocomplete = (account: number, query: string) =>
  invoke("autocomplete", { account, query });
export const addAccount = () => invoke("add_account", {});
export const signInAgain = (account: number) => invoke("sign_in_again", { account });
export const openAttachment = (attachment: number) => invoke("open_attachment", { attachment });
export const quitNow = () => invoke("quit_now", {});
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
