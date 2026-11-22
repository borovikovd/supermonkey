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

export function currentListView(): ListView | null {
  const key = currentListKey();
  const id = key ? subIdFor(key) : undefined;
  return id ? viewList(useMirror.getState(), id) : null;
}

export function visibleIds(): string[] {
  return currentListView()?.ids.map((e) => e.key) ?? [];
}

export function currentListLoaded(): boolean {
  const key = currentListKey();
  const id = key ? subIdFor(key) : undefined;
  const sub = id ? useMirror.getState().subs[id] : undefined;
  return sub?.type === "list" && sub.loaded;
}

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

export function openConversationData(): ConversationData | null {
  const screen = useChrome.getState().current()?.screen;
  if (screen?.kind !== "conversation") return null;
  return conversationData(useMirror.getState(), screen.thread, screen.trashMode);
}

export function splitIdOf(kind: "important" | "other"): number | null {
  const { account } = useChrome.getState();
  if (account === null) return null;
  const summary = accountSummary(useMirror.getState(), account);
  return summary?.splits.find((s) => s.kind === kind && s.enabled)?.id ?? null;
}
