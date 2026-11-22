import { useMemo } from "react";

import { useChrome } from "../store/chrome";
import { listKey } from "../store/current";
import { useCurrent, useListView, useSummary } from "../store/hooks";

export function useCurrentList() {
  const { account, cur } = useCurrent();
  return useListView(account !== null && cur ? listKey(account, cur.where) : null);
}

export function useSplitCounts() {
  const { account } = useCurrent();
  const summary = useSummary(account);
  const list = useCurrentList();
  const forced = useChrome((s) => s.forcedSplit);
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
