import { useEffect, useMemo, useState } from "react";

import { keyString, subscribe, unsubscribe } from "../ipc/bridge";
import type { AccountSummary, AppState, ConversationData, DraftState, SubKey } from "../ipc/types";
import { useChrome, type Where } from "./chrome";
import { accountSummary, appState, viewList, type ListView, type MirrorState } from "./mirror-core";
import { useMirror } from "./mirror";

/**
 * The mirror's subscription id for a key. Read from the store, not the bridge's
 * map, so a component re-renders when the subscription opens or closes.
 */
export function useSubId(key: SubKey | null): string | undefined {
  const k = key ? keyString(key) : null;
  return useMirror((s) => {
    if (k === null) return undefined;
    for (const [id, sub] of Object.entries(s.subs)) if (keyString(sub.key) === k) return id;
    return undefined;
  });
}

/** Keeps a subscription open while the component is mounted. */
export function useSubscription(key: SubKey | null): string | undefined {
  // Callers build a fresh key object on every render; keep one per value.
  const [stable, setStable] = useState(key);
  const k = key ? keyString(key) : null;
  if (k !== (stable ? keyString(stable) : null)) setStable(key);
  useEffect(() => {
    if (!stable) return;
    subscribe(stable);
    return () => unsubscribe(stable);
  }, [stable]);
  return useSubId(key);
}

export function useAppState(): AppState | null {
  return useMirror(appState);
}

/** The account needs Sign in again: its screen shows that, not its mail. */
export function needsSignin(account: number | null): boolean {
  return signedOut(useMirror.getState(), account);
}

export function useNeedsSignin(account: number | null): boolean {
  return useMirror((s) => signedOut(s, account));
}

function signedOut(s: MirrorState, account: number | null): boolean {
  return appState(s)?.accounts.find((a) => a.id === account)?.needsSignin ?? false;
}

export function useSummary(account: number | null): AccountSummary | null {
  return useMirror((s) => (account === null ? null : accountSummary(s, account)));
}

export function listKey(account: number, where: Where): SubKey {
  return where.kind === "search"
    ? { type: "search", account, query: where.query }
    : { type: "list", account, view: where.view };
}

/** The list as shown, with pending actions applied. */
export function useListView(
  key: SubKey | null,
): ListView & { loaded: boolean; searching: boolean; more: boolean; sub?: string } {
  const id = useSubId(key);
  const sub = useMirror((s) => (id ? s.subs[id] : undefined));
  const rows = useMirror((s) => s.rows);
  const pending = useMirror((s) => s.pending);
  const removals = useMirror((s) => s.removals);
  // Explicit memo, not left to the compiler: `ids` feeds effect dependencies in
  // ListView, and the compiler's caching is an optimization, not a guarantee.
  return useMemo(() => {
    if (!id || sub?.type !== "list") {
      return { ids: [], rows: {}, loaded: false, searching: false, more: false };
    }
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
      if (sub.type === "composer" && sub.key.draft === draft) return sub.draft;
    }
    return null;
  });
}

export function useCurrent() {
  const account = useChrome((s) => s.account);
  const cur = useChrome((s) => (s.account === null ? null : s.perAccount[s.account]));
  return { account, cur };
}
