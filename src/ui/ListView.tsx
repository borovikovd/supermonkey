import { useVirtualizer } from "@tanstack/react-virtual";
import { Paperclip, Star } from "lucide-react";
import { memo, useCallback, useEffect, useMemo, useRef } from "react";

import { markReadOnOpenWhenLoaded, openThread } from "../commands";
import { startReply } from "../compose/start";
import { act, fetchRows } from "../ipc/bridge";
import type { ListRow } from "../ipc/types";
import { appHooks } from "../keyboard/refs";
import { useChrome } from "../store/chrome";
import { useCurrent } from "../store/hooks";
import { useMirror } from "../store/mirror";
import { useCurrentList, useSplitCounts } from "./current-list";
import { countLabel, formatTime } from "./format";

const ROW = 36;
const HEADER = 28;

type Item =
  | { type: "row"; key: string }
  | { type: "header"; split: number; name: string; count: number };

// Memoized by hand: ListView is not compiled (see useVirtualizer) and
// re-renders on every scroll.
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
  const bg = selected ? "bg-fill" : highlighted ? "bg-accent-soft" : "hover:bg-fill-half";
  return (
    // An ARIA listbox option: focus stays on the listbox (aria-activedescendant)
    // and j / k / Enter drive it through the global shortcut handler. Rows are
    // rich and virtualized, so a native <option> cannot render them.
    // oxlint-disable-next-line jsx-a11y/click-events-have-key-events, jsx-a11y/interactive-supports-focus
    <div
      // oxlint-disable-next-line jsx-a11y/prefer-tag-over-role -- rich rows; native elements cannot render them
      role="option"
      aria-selected={highlighted}
      id={`row-${rowKey}`}
      onClick={() => onClick(rowKey)}
      className={`relative flex h-row items-center gap-3 px-4 text-row ${bg}`}
      data-row={rowKey}
      data-selected={selected ? true : undefined}
      data-unread={row?.unread ? true : undefined}
      data-starred={row?.starred ? true : undefined}
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

function SplitHeader({ split, name, count }: { split: number; name: string; count: number }) {
  return (
    <h2
      data-split={split}
      aria-label={count > 0 ? `${name} ${countLabel(count)}` : name}
      className="flex h-split-header items-end gap-1.5 px-4 pb-1 text-hint"
    >
      <span className="font-medium text-text-2">{name}</span>
      {count > 0 && <span className="tabular text-text-3">{countLabel(count)}</span>}
    </h2>
  );
}

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

/** Row keys asked for and not answered yet: scrolling never asks twice. */
const fetching = new Set<string>();

export function ListView() {
  const { cur } = useCurrent();
  const list = useCurrentList();
  const selection = useChrome((s) => s.selection);
  const splits = useSplitCounts();
  const viewKind = cur?.where.kind === "view" ? cur.where.view.kind : null;
  const isInbox = viewKind === "inbox";
  const isDrafts = viewKind === "drafts";
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
  // TanStack Virtual returns functions the React Compiler cannot memoize, so it
  // skips this component; the list is virtualized and stays fast without it.
  // oxlint-disable-next-line react/incompatible-library
  const v = useVirtualizer({
    count: items.length,
    getScrollElement: () => parent.current,
    estimateSize: (i) => (items[i]?.type === "header" ? HEADER : ROW),
    overscan: 20,
    getItemKey: (i) => {
      const it = items[i];
      if (!it) return i;
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
    const top = list.ids[0];
    if (highlight === null && top && cur?.screen.kind === "list") {
      useChrome.getState().patchCurrent({ highlight: top.key });
    }
  }, [highlight, list.ids, cur?.screen.kind]);

  // A click on a split name asks for its header at the top.
  const headerToTop = useRef(false);
  useEffect(() => {
    appHooks.headerToTop = () => {
      headerToTop.current = true;
    };
  }, []);

  // Keep the highlight in view.
  useEffect(() => {
    if (highlight === null) return;
    const i = items.findIndex((it) => it.type === "row" && it.key === highlight);
    if (i < 0) return;
    const toTop = headerToTop.current;
    headerToTop.current = false;
    // Show the split header too when the highlight is its first row. Only a
    // header above the viewport needs scrolling to: scrolling down to the row
    // brings the header just above it into view anyway.
    const header = items[i - 1]?.type === "header" ? v.measurementsCache[i - 1] : undefined;
    if (header && (toTop || header.start < (v.scrollOffset ?? 0)))
      v.scrollToIndex(i - 1, { align: "start" });
    else v.scrollToIndex(i, { align: "auto" });
  }, [highlight, items, v]);

  // Rows outside the pushed window are fetched ahead of scrolling.
  const virtual = v.getVirtualItems();
  const rows = list.rows;
  useEffect(() => {
    const head = virtual[0];
    const tail = virtual.at(-1);
    if (!head || !tail) return;
    const first = Math.max(0, head.index - 40);
    const last = Math.min(items.length - 1, tail.index + 60);
    const missing: string[] = [];
    for (let i = first; i <= last; i++) {
      const it = items[i];
      if (it?.type === "row" && !rows[it.key] && !fetching.has(it.key)) missing.push(it.key);
    }
    if (missing.length === 0) return;
    const t = setTimeout(() => {
      for (const k of missing) fetching.add(k);
      fetchRows(isDrafts, missing)
        .then((r) => useMirror.getState().putRows(r))
        .catch((err: unknown) => console.warn("fetch list rows", err))
        .finally(() => {
          for (const k of missing) fetching.delete(k);
        });
    }, 0);
    return () => clearTimeout(t);
  }, [virtual, items, rows, isDrafts]);

  // Gmail pages: fetch the next one as the list nears its end.
  const asked = useRef(0);
  useEffect(() => {
    const tail = virtual.at(-1);
    if (!list.more || !list.sub || !tail) return;
    const last = tail.index;
    if (last >= items.length - 15 && asked.current !== items.length) {
      asked.current = items.length;
      act({ type: "moreResults", sub: list.sub });
    }
  }, [virtual, items.length, list.more, list.sub]);

  const onClick = useCallback(
    (k: string) => {
      useChrome.getState().patchCurrent({ highlight: k });
      if (isDrafts) {
        startReply("resumeDraft", k);
        return;
      }
      openThread(Number(k));
      markReadOnOpenWhenLoaded(Number(k));
    },
    [isDrafts],
  );

  const selected = useMemo(() => new Set(selection), [selection]);

  if (items.length === 0) {
    return (
      <div className="flex flex-1 flex-col">
        {cur?.where.kind === "search" && list.searching && (
          <div className="px-4 py-3 text-row text-text-3">Searching Gmail…</div>
        )}
        <Empty inbox={isInbox} loaded={list.loaded && !list.searching} />
      </div>
    );
  }

  return (
    <div ref={parent} className="min-h-0 flex-1 overflow-y-auto" style={{ contain: "strict" }}>
      <div
        // oxlint-disable-next-line jsx-a11y/prefer-tag-over-role -- rich rows; native elements cannot render them
        role="listbox"
        tabIndex={0}
        aria-label="Conversations"
        aria-activedescendant={highlight === null ? undefined : `row-${highlight}`}
        style={{ height: v.getTotalSize(), position: "relative" }}
      >
        {virtual.map((vi) => {
          const it = items[vi.index];
          if (!it) return null;
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
                <SplitHeader split={it.split} name={it.name} count={it.count} />
              ) : (
                <Row
                  row={rows[it.key]}
                  rowKey={it.key}
                  highlighted={it.key === highlight}
                  selected={selected.has(it.key)}
                  onClick={onClick}
                />
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
