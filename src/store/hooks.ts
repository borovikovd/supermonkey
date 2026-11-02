import { useEffect, useMemo } from "react";

import { keyString, subIdFor, subscribe, unsubscribe } from "../ipc/bridge";
import type { AccountSummary, AppState, ConversationData, DraftState, SubKey } from "../ipc/types";
import { useChrome, type Where } from "./chrome";
import { viewList, type ListView } from "./mirror-core";
import { useMirror } from "./mirror";

/** Keeps a subscription open while the component is mounted. */
export function useSubscription(key: SubKey | null): string | undefined {
  const k = key ? keyString(key) : null;
  useEffect(() => {
    if (!key) return;
    subscribe(key);
    return () => unsubscribe(key);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [k]);
  return key ? subIdFor(key) : undefined;
}

export function useAppState(): AppState | null {
  return useMirror((s) => {
    for (const sub of Object.values(s.subs)) if (sub.type === "app") return sub.state;
    return null;
  });
}

export function useSummary(account: number | null): AccountSummary | null {
  return useMirror((s) => {
    if (account === null) return null;
    for (const sub of Object.values(s.subs)) {
      if (sub.type === "account" && sub.key.type === "account" && sub.key.account === account)
        return sub.summary;
    }
    return null;
  });
}

export function listKey(account: number, where: Where): SubKey {
  return where.kind === "search"
    ? { type: "search", account, query: where.query }
    : { type: "list", account, view: where.view };
}

const EMPTY: ListView = { ids: [], rows: {} };

/** The list as shown, with pending actions applied. */
export function useListView(
  key: SubKey | null,
): ListView & { loaded: boolean; searching: boolean; more: boolean; sub?: string } {
  const id = key ? subIdFor(key) : undefined;
  const sub = useMirror((s) => (id ? s.subs[id] : undefined));
  const rows = useMirror((s) => s.rows);
  const pending = useMirror((s) => s.pending);
  const removals = useMirror((s) => s.removals);
  return useMemo(() => {
    if (!id || !sub || sub.type !== "list")
      return { ...EMPTY, loaded: false, searching: false, more: false };
    const view = viewList({ ack: 0, rev: 0, subs: { [id]: sub }, rows, pending, removals }, id);
    return { ...view, loaded: sub.loaded, searching: sub.searching, more: sub.more, sub: id };
  }, [id, sub, rows, pending, removals]);
}

export function useConversation(
  thread: number | null,
  trashMode: boolean,
): ConversationData | null {
  return useMirror((s) => {
    if (thread === null) return null;
    let prefetched: ConversationData | null = null;
    for (const sub of Object.values(s.subs)) {
      if (
        sub.type === "conversation" &&
        sub.key.type === "conversation" &&
        sub.key.thread === thread &&
        sub.key.trashMode === trashMode &&
        sub.data
      ) {
        if (sub.key.open) return sub.data;
        prefetched = sub.data;
      }
    }
    return prefetched;
  });
}

export function useDraft(draft: string | null): DraftState | null {
  return useMirror((s) => {
    if (!draft) return null;
    for (const sub of Object.values(s.subs)) {
      if (sub.type === "composer" && sub.key.type === "composer" && sub.key.draft === draft)
        return sub.draft;
    }
    return null;
  });
}

export function useCurrent() {
  const account = useChrome((s) => s.account);
  const cur = useChrome((s) => (s.account === null ? null : s.perAccount[s.account]));
  return { account, cur };
}
