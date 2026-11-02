import { useMemo } from "react";

import { useChrome } from "../store/chrome";
import { listKey, useCurrent, useListView, useSummary } from "../store/hooks";

/** The list on screen for the current account: its view or its search. */
export function useCurrentList() {
  const { account, cur } = useCurrent();
  return useListView(account !== null && cur ? listKey(account, cur.where) : null);
}

/** Split names with counts for the Inbox header and section headers. */
export function useSplitCounts() {
  const { account } = useCurrent();
  const summary = useSummary(account);
  const list = useCurrentList();
  const forced = useChrome((s) => s.forcedSplit);
  // Memoized by hand: ListView, which the compiler skips, keys its effects off
  // this array's identity.
  return useMemo(() => {
    const counts = new Map<number, number>();
    for (const e of list.ids)
      if (e.split !== null) counts.set(e.split, (counts.get(e.split) ?? 0) + 1);
    return (summary?.splits ?? [])
      .filter((s) => s.enabled || s.kind === "other")
      .map((s) => ({
        id: s.id,
        name: s.name,
        hideWhenEmpty: s.hideWhenEmpty,
        count: counts.get(s.id) ?? 0,
      }))
      .filter((s) => s.count > 0 || !s.hideWhenEmpty || s.id === forced);
  }, [list.ids, summary, forced]);
}
