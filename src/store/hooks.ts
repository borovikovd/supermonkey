import { useEffect, useMemo, useState } from "react";

import { keyString, subIdFor, subscribe, unsubscribe } from "../ipc/bridge";
import type { AccountSummary, AppState, ConversationData, DraftState, SubKey } from "../ipc/types";
import { useChrome } from "./chrome";
import { conversationData } from "./current";
import {
  accountSummary,
  appState,
  composerDraft,
  viewList,
  type ListView,
  type MirrorState,
} from "./mirror-core";
import { useMirror } from "./mirror";

function useSubId(key: SubKey | null): string | undefined {
  return useMirror((s) => {
    const id = key ? subIdFor(key) : undefined;
    return id !== undefined && id in s.subs ? id : undefined;
  });
}

export function useSubscription(key: SubKey | null): string | undefined {
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

export function useNeedsSignin(account: number | null): boolean {
  return useMirror((s) => signedOut(s, account));
}

function signedOut(s: MirrorState, account: number | null): boolean {
  return appState(s)?.accounts.find((a) => a.id === account)?.needsSignin ?? false;
}

export function useSummary(account: number | null): AccountSummary | null {
  return useMirror((s) => (account === null ? null : accountSummary(s, account)));
}

export function useListView(key: SubKey | null): ListView & {
  loaded: boolean;
  searching: boolean;
  needsGmail: boolean;
  terms: string[];
  more: boolean;
  oldestHit: string | null;
  sub?: string;
} {
  const id = useSubId(key);
  const sub = useMirror((s) => (id ? s.subs[id] : undefined));
  const rows = useMirror((s) => s.rows);
  const pending = useMirror((s) => s.pending);
  const removals = useMirror((s) => s.removals);
  return useMemo(() => {
    if (!id || sub?.type !== "list") {
      return {
        ids: [],
        rows: {},
        loaded: false,
        searching: false,
        needsGmail: false,
        terms: [],
        more: false,
        oldestHit: null,
      };
    }
    const view = viewList({ ack: 0, subs: { [id]: sub }, rows, pending, removals }, id);
    return {
      ...view,
      loaded: sub.loaded,
      searching: sub.searching,
      needsGmail: sub.needsGmail,
      terms: sub.terms,
      more: sub.more,
      oldestHit: sub.oldestHit,
      sub: id,
    };
  }, [id, sub, rows, pending, removals]);
}

export function useConversation(
  thread: number | null,
  trashMode: boolean,
): ConversationData | null {
  return useMirror((s) => (thread === null ? null : conversationData(s, thread, trashMode)));
}

export function useDraft(draft: string | null): DraftState | null {
  return useMirror((s) => (draft ? composerDraft(s, draft) : null));
}

export function useCurrent() {
  const account = useChrome((s) => s.account);
  const cur = useChrome((s) => (s.account === null ? null : s.perAccount[s.account]));
  return { account, cur };
}
