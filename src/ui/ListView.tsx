import { useVirtualizer } from "@tanstack/react-virtual";
import { Paperclip, Star } from "lucide-react";
import { memo, useEffect, useMemo, useRef } from "react";

import { markReadOnOpenWhenLoaded, openThread } from "../commands";
import { startReply } from "../compose/start";
import { act, fetchRows } from "../ipc/bridge";
import type { ListRow } from "../ipc/types";
import { useChrome } from "../store/chrome";
import { listKey, useCurrent, useListView, useSummary } from "../store/hooks";
import { useMirror } from "../store/mirror";
import { countLabel, useSplitCounts } from "./TopBar";

const ROW = 36;
const HEADER = 28;

export function formatTime(ms: number): string {
  const d = new Date(ms);
  const now = new Date();
  if (d.toDateString() === now.toDateString()) {
    return d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
  }
  if (d.getFullYear() === now.getFullYear()) {
    return d.toLocaleDateString(undefined, { month: "short", day: "numeric" });
  }
  return d.toLocaleDateString(undefined, { year: "2-digit", month: "numeric", day: "numeric" });
}

type Item =
  | { type: "row"; key: string }
  | { type: "header"; split: number; name: string; count: number };

const Row = memo(function Row({
  row,
  rowKey,
  highlighted,
  selected,
  onClick,
}: {
  row: ListRow | undefined;
  rowKey: string;
  highlighted: boolean;
  selected: boolean;
  onClick: (key: string) => void;
}) {
  const bg =
    highlighted && selected
      ? "bg-fill"
      : highlighted
        ? "bg-accent-soft"
        : selected
          ? "bg-fill"
          : "hover:bg-fill-half";
  return (
    <div
      onClick={() => onClick(rowKey)}
      className={`relative flex h-row items-center gap-3 px-4 text-row ${bg}`}
      data-row={rowKey}
    >
      {highlighted && <span className="absolute top-0 bottom-0 left-0 w-[3px] bg-accent" />}
      <span className="flex w-[5px] shrink-0 justify-center">
        {row?.unread && <span className="h-[5px] w-[5px] rounded-full bg-unread" />}
      </span>
      <span
        className={`w-[180px] shrink-0 truncate ${row?.unread ? "font-semibold text-text" : "font-medium text-text-2"}`}
      >
        {row ? row.sender || row.senderEmail : ""}
        {row && row.messageCount > 1 && (
          <span className="ml-1 font-normal text-text-3">{row.messageCount}</span>
        )}
      </span>
      <span className="flex min-w-0 flex-1 items-center gap-1 truncate">
        {row?.starred && (
          <Star size={12} strokeWidth={1.5} className="shrink-0 fill-[#E8B339] text-[#E8B339]" />
        )}
        <span className="truncate text-text">
          {row?.subject || (row ? "(no subject)" : "")}
          <span className="text-text-2">{row?.snippet ? ` — ${row.snippet}` : ""}</span>
        </span>
      </span>
      {row?.hasAttachments && (
        <Paperclip size={12} strokeWidth={1.5} className="shrink-0 text-text-3" />
      )}
      <span className="tabular shrink-0 text-time text-text-3">
        {row ? formatTime(row.snoozeUntil ?? row.time) : ""}
      </span>
    </div>
  );
});

function Empty({ inbox, loaded }: { inbox: boolean; loaded: boolean }) {
  if (!loaded) return null;
  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-1 text-text-2">
      <span className="text-zero font-medium text-text">
        {inbox ? "You're all done." : "Nothing here."}
      </span>
      {inbox && <span className="text-row">Inbox zero.</span>}
    </div>
  );
}

export function ListView() {
  const { account, cur } = useCurrent();
  const key = account !== null && cur ? listKey(account, cur.where) : null;
  const list = useListView(key);
  const selection = useChrome((s) => s.selection);
  const summary = useSummary(account);
  const splits = useSplitCounts(account);
  const isInbox = cur?.where.kind === "view" && cur.where.view.kind === "inbox";
  const isDrafts = cur?.where.kind === "view" && cur.where.view.kind === "drafts";
  const highlight = cur?.highlight ?? null;
  const forced = useChrome((s) => s.forcedSplit);

  const items: Item[] = useMemo(() => {
    if (!isInbox) return list.ids.map((e) => ({ type: "row", key: e.key }));
    const out: Item[] = [];
    let last: number | null | undefined;
    const showHeaders = splits.length > 1;
    for (const e of list.ids) {
      if (showHeaders && e.split !== last) {
        const s = splits.find((x) => x.id === e.split);
        if (s) out.push({ type: "header", split: s.id, name: s.name, count: s.count });
        last = e.split;
      }
      out.push({ type: "row", key: e.key });
    }
    // `g i` / `g o` on an empty, hidden split shows its header.
    if (forced !== null && !list.ids.some((e) => e.split === forced)) {
      const s = splits.find((x) => x.id === forced);
      if (s) out.unshift({ type: "header", split: s.id, name: s.name, count: 0 });
    }
    return out;
  }, [list.ids, isInbox, splits, forced]);

  const parent = useRef<HTMLDivElement>(null);
  const v = useVirtualizer({
    count: items.length,
    getScrollElement: () => parent.current,
    estimateSize: (i) => (items[i]?.type === "header" ? HEADER : ROW),
    overscan: 20,
    getItemKey: (i) => {
      const it = items[i]!;
      return it.type === "row" ? it.key : `h${it.split}`;
    },
  });

  // Debug builds: when the first rows paint after launch (cold launch budget).
  useEffect(() => {
    if (items.length > 0 && performance.getEntriesByName("first-rows").length === 0) {
      requestAnimationFrame(() => performance.mark("first-rows"));
    }
  }, [items.length]);

  // First visit: the top row is highlighted.
  useEffect(() => {
    if (highlight === null && list.ids.length > 0 && cur?.screen.kind === "list") {
      useChrome.getState().patchCurrent({ highlight: list.ids[0]!.key });
    }
  }, [highlight, list.ids, cur?.screen.kind]);

  // Keep the highlight in view.
  useEffect(() => {
    if (highlight === null) return;
    const i = items.findIndex((it) => it.type === "row" && it.key === highlight);
    if (i >= 0) {
      // Show the split header too when the highlight is its first row.
      const prev = items[i - 1];
      v.scrollToIndex(prev?.type === "header" ? i - 1 : i, { align: "auto" });
    }
  }, [highlight, items, v]);

  // Rows outside the pushed window are fetched ahead of scrolling.
  const virtual = v.getVirtualItems();
  const rows = list.rows;
  useEffect(() => {
    if (virtual.length === 0) return;
    const first = Math.max(0, virtual[0]!.index - 40);
    const last = Math.min(items.length - 1, virtual[virtual.length - 1]!.index + 60);
    const missing: string[] = [];
    for (let i = first; i <= last; i++) {
      const it = items[i];
      if (it?.type === "row" && !rows[it.key]) missing.push(it.key);
    }
    if (missing.length === 0) return;
    const t = setTimeout(() => {
      void fetchRows(isDrafts, missing).then((r) => useMirror.getState().putRows(r));
    }, 0);
    return () => clearTimeout(t);
  }, [virtual, items, rows, isDrafts]);

  // Gmail pages: fetch the next one as the list nears its end.
  const asked = useRef(0);
  useEffect(() => {
    if (!list.more || !list.sub || virtual.length === 0) return;
    const last = virtual[virtual.length - 1]!.index;
    if (last >= items.length - 15 && asked.current !== items.length) {
      asked.current = items.length;
      act({ type: "moreResults", sub: list.sub });
    }
  }, [virtual, items.length, list.more, list.sub]);

  const onClick = useMemo(
    () => (k: string) => {
      const c = useChrome.getState();
      c.patchCurrent({ highlight: k });
      if (isDrafts) {
        startReply("resumeDraft", k);
        return;
      }
      openThread(Number(k));
      markReadOnOpenWhenLoaded(Number(k));
    },
    [isDrafts],
  );

  const sel = useMemo(() => new Set(selection), [selection]);
  const searching = list.searching;

  if (items.length === 0) {
    return (
      <div className="flex flex-1 flex-col">
        {cur?.where.kind === "search" && searching && (
          <div className="px-4 py-3 text-row text-text-3">Searching Gmail…</div>
        )}
        <Empty inbox={isInbox} loaded={list.loaded && !searching} />
      </div>
    );
  }

  return (
    <div ref={parent} className="min-h-0 flex-1 overflow-y-auto" style={{ contain: "strict" }}>
      <div style={{ height: v.getTotalSize(), position: "relative" }}>
        {virtual.map((vi) => {
          const it = items[vi.index]!;
          return (
            <div
              key={vi.key}
              style={{
                position: "absolute",
                top: 0,
                left: 0,
                right: 0,
                transform: `translateY(${vi.start}px)`,
              }}
            >
              {it.type === "header" ? (
                <div
                  data-split={it.split}
                  className="flex h-split-header items-end gap-1.5 px-4 pb-1 text-hint"
                >
                  <span className="font-medium text-text-2">{it.name}</span>
                  {it.count > 0 && (
                    <span className="tabular text-text-3">{countLabel(it.count)}</span>
                  )}
                </div>
              ) : (
                <Row
                  row={rows[it.key]}
                  rowKey={it.key}
                  highlighted={it.key === highlight}
                  selected={sel.has(it.key)}
                  onClick={onClick}
                />
              )}
            </div>
          );
        })}
      </div>
      {summary === null && null}
    </div>
  );
}
