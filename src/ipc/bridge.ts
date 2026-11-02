// The UI's only door to Rust: numbered actions sent one invoke at a time,
// subscriptions, and a few plain commands.

import { Channel, invoke as tauriInvoke } from "@tauri-apps/api/core";

import { subForKey } from "../store/mirror-core";
import { useMirror } from "../store/mirror";
import type {
  AccountId,
  Action,
  ChannelMessage,
  ContactSuggestion,
  LaunchInfo,
  ListRow,
  MessageBody,
  NumberedAction,
  SnoozeChoice,
  SubKey,
} from "./bridge-types";

/** Inside the Tauri webview; otherwise the debug web bridge on 127.0.0.1:1430. */
export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
const BRIDGE = "http://127.0.0.1:1430";

async function invoke<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> {
  if (inTauri) return tauriInvoke<T>(cmd, args);
  const r = await fetch(`${BRIDGE}/invoke/${cmd}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(args),
  });
  const body = (await r.json()) as T;
  if (!r.ok) throw new Error(String(body));
  return body;
}

let nextN = 1;
let buffer: NumberedAction[] = [];
let inFlight = false;
export let launch: LaunchInfo = { lastAction: 0, debug: false } as LaunchInfo;

async function flush(): Promise<void> {
  if (inFlight || buffer.length === 0) return;
  inFlight = true;
  const actions = buffer;
  buffer = [];
  try {
    await invoke("submit", { actions });
  } finally {
    inFlight = false;
    void flush();
  }
}

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

export const keyString = (key: SubKey) => JSON.stringify(key);

/** Subscribes (ref-counted by key) and returns the subscription id. */
export function subscribe(key: SubKey): string {
  const k = keyString(key);
  const have = byKey.get(k);
  if (have) {
    have.refs++;
    return have.id;
  }
  const id = `s${++subSeq}`;
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

// ------------------------------------------------------------ startup

export async function connect(): Promise<LaunchInfo> {
  if (inTauri) {
    const channel = new Channel<ChannelMessage>();
    channel.onmessage = (msg) => useMirror.getState().receive(msg);
    launch = await invoke<LaunchInfo>("connect", { channel });
  } else {
    const events = new EventSource(`${BRIDGE}/events`);
    events.onmessage = (e) =>
      useMirror.getState().receive(JSON.parse(e.data as string) as ChannelMessage);
    await new Promise<void>((resolve) => {
      events.onopen = () => resolve();
    });
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
export const perfReport = (action: string, ms: number, budget: number) =>
  invoke<void>("perf_report", { action, ms, budget });
export const snoozeOptions = (text: string) => invoke<SnoozeChoice[]>("snooze_options", { text });
