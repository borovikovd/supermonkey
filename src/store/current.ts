// What is on screen, read from the chrome store and the mirror outside React:
// the current list, its rows, and the open conversation's data.

import { subIdFor } from "../ipc/bridge";
import type { ConversationData, SubKey, View } from "../ipc/types";
import { useChrome, type Where } from "./chrome";
import { accountSummary, viewList, type ListView, type MirrorState } from "./mirror-core";
import { useMirror } from "./mirror";

export function listKey(account: number, where: Where): SubKey {
  return where.kind === "search"
    ? { type: "search", account, query: where.query }
    : { type: "list", account, view: where.view };
}

export function currentListKey(): SubKey | null {
  const c = useChrome.getState();
  const cur = c.current();
  return c.account === null || !cur ? null : listKey(c.account, cur.where);
}

export function currentView(): View | null {
  const where = useChrome.getState().current()?.where;
  return where?.kind === "view" ? where.view : null;
}

/** The list on screen with pending actions applied, once it is subscribed. */
export function currentListView(): ListView | null {
  const key = currentListKey();
  const id = key ? subIdFor(key) : undefined;
  return id ? viewList(useMirror.getState(), id) : null;
}

export function visibleIds(): string[] {
  return currentListView()?.ids.map((e) => e.key) ?? [];
}

/** Whether the list on screen has had its first load, so its rows are the truth. */
export function currentListLoaded(): boolean {
  const key = currentListKey();
  const id = key ? subIdFor(key) : undefined;
  const sub = id ? useMirror.getState().subs[id] : undefined;
  return sub?.type === "list" && sub.loaded;
}

/**
 * Whether a conversation opened from the list on screen lists its messages in
 * trash and spam: from Trash or Spam, or from a search that looks there
 * (SPEC Reading).
 */
export function listShowsTrash(): boolean {
  const where = useChrome.getState().current()?.where;
  if (where?.kind === "view") return where.view.kind === "trash" || where.view.kind === "spam";
  const key = currentListKey();
  const id = key ? subIdFor(key) : undefined;
  const sub = id ? useMirror.getState().subs[id] : undefined;
  return sub?.type === "list" && sub.looksInTrash;
}

export function rowOf(key: string) {
  return (currentListView()?.rows ?? useMirror.getState().rows)[key];
}

/** A conversation's data: the open subscription's, else a prefetched one's. */
export function conversationData(
  s: MirrorState,
  thread: number,
  trashMode: boolean,
): ConversationData | null {
  let prefetched: ConversationData | null = null;
  for (const sub of Object.values(s.subs)) {
    if (
      sub.type === "conversation" &&
      sub.key.thread === thread &&
      sub.key.trashMode === trashMode &&
      sub.data
    ) {
      if (sub.key.open) return sub.data;
      prefetched = sub.data;
    }
  }
  return prefetched;
}

/** The open conversation's data, once loaded. */
export function openConversationData(): ConversationData | null {
  const screen = useChrome.getState().current()?.screen;
  if (screen?.kind !== "conversation") return null;
  return conversationData(useMirror.getState(), screen.thread, screen.trashMode);
}

/** The enabled split of this kind in the account on screen. */
export function splitIdOf(kind: "important" | "other"): number | null {
  const { account } = useChrome.getState();
  if (account === null) return null;
  const summary = accountSummary(useMirror.getState(), account);
  return summary?.splits.find((s) => s.kind === kind && s.enabled)?.id ?? null;
}
