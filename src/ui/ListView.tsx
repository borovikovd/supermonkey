import { useVirtualizer } from "@tanstack/react-virtual";
import { Paperclip, Star } from "lucide-react";
import { memo, useCallback, useEffect, useMemo, useRef } from "react";

import { resumeDraftById } from "../compose/start";
import { act, fetchRows } from "../ipc/bridge";
import type { ListRow } from "../ipc/types";
import { listScroll } from "../keyboard/refs";
import { highlight as setHighlight, openConversation } from "../navigation";
import { useChrome } from "../store/chrome";
import { marked } from "./highlight";
import { useAppState, useCurrent } from "../store/hooks";
import { useMirror } from "../store/mirror";
import { useCurrentList, useSplitCounts } from "./current-list";
import { countLabel, formatTime } from "./format";

const ROW = 36;
const HEADER = 28;

type Item =
  | { type: "row"; key: string }
  | { type: "header"; split: number; name: string; count: number };

const pointer = { x: -1, y: -1 };
if (typeof window !== "undefined") {
  window.addEventListener("mousemove", (e) => {
    pointer.x = e.clientX;
    pointer.y = e.clientY;
  });
}

const Row = memo(function Row({
  row,
  rowKey,
  highlighted,
  selected,
  onClick,
  onHover,
  terms,
}: {
  row: ListRow | undefined;
  rowKey: string;
  highlighted: boolean;
  selected: boolean;
  terms: readonly string[];
  onClick: (key: string) => void;
  onHover: (key: string, e: React.MouseEvent) => void;
}) {
  const bg = selected ? "bg-fill" : highlighted ? "bg-accent-soft" : "";
  return (
    // oxlint-disable-next-line jsx-a11y/click-events-have-key-events, jsx-a11y/interactive-supports-focus -- a listbox option; keys drive it through the global handler
    <div
      // oxlint-disable-next-line jsx-a11y/prefer-tag-over-role -- rich rows; native elements cannot render them
      role="option"
      aria-selected={highlighted}
      id={`row-${rowKey}`}
      onClick={() => onClick(rowKey)}
      onMouseMove={(e) => onHover(rowKey, e)}
      className={`relative flex h-row items-center gap-3 px-4 text-row ${bg}`}
      data-row={rowKey}
      data-selected={selected ? true : undefined}
      data-unread={row?.unread ? true : undefined}
      data-starred={row?.starred ? true : undefined}
      data-attachments={row?.hasAttachments ? true : undefined}
    >
      {highlighted && <span className="absolute top-0 bottom-0 left-0 w-0.75 bg-accent" />}
      <span className="flex w-1.25 shrink-0 justify-center">
        {row?.unread && <span className="size-1.25 rounded-full bg-unread" />}
      </span>
      <span
        className={`w-sender shrink-0 truncate ${row?.unread ? "font-semibold text-text" : "font-medium text-text-2"}`}
      >
        {row ? row.sender || row.senderEmail : ""}
        {row && row.messageCount > 1 && (
          <span className="ml-1 font-normal text-text-3">{row.messageCount}</span>
        )}
      </span>
      <span className="flex min-w-0 flex-1 items-center gap-1 truncate">
        {row?.starred && (
          <Star size={12} strokeWidth={1.5} className="shrink-0 fill-star text-star" />
        )}
        <span className="truncate text-text">
          <span>{row?.subject ? marked(row.subject, terms) : row ? "(no subject)" : ""}</span>
          {row?.snippet && <span className="text-text-2"> — {marked(row.snippet, terms)}</span>}
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
  const online = useAppState()?.online ?? true;

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
    if (forced !== null && !list.ids.some((e) => e.split === forced)) {
      const s = splits.find((x) => x.id === forced);
      if (s) out.unshift({ type: "header", split: s.id, name: s.name, count: 0 });
    }
    return out;
  }, [list.ids, isInbox, splits, forced]);

  const parent = useRef<HTMLDivElement>(null);
  // oxlint-disable-next-line react/incompatible-library -- TanStack Virtual; the list is fast without the compiler
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

  useEffect(() => {
    if (items.length > 0 && performance.getEntriesByName("first-rows").length === 0) {
      requestAnimationFrame(() => performance.mark("first-rows"));
    }
  }, [items.length]);

  const hovered = useRef<string | null>(null);
  const shown = useRef<string | null>(null);
  const listSub = list.sub;
  useEffect(() => {
    const target = highlight === null ? null : `${listSub ?? ""}:${highlight}`;
    if (target === shown.current) return;
    const fromPointer = hovered.current === highlight;
    hovered.current = null;
    if (highlight === null || fromPointer) {
      shown.current = target;
      return;
    }
    const i = items.findIndex((it) => it.type === "row" && it.key === highlight);
    if (i < 0) return;
    shown.current = target;
    const toTop = listScroll.headerToTop;
    listScroll.headerToTop = false;
    const header = items[i - 1]?.type === "header" ? v.measurementsCache[i - 1] : undefined;
    if (header && (toTop || header.start < (v.scrollOffset ?? 0)))
      v.scrollToIndex(i - 1, { align: "start" });
    else v.scrollToIndex(i, { align: "auto" });
  }, [highlight, items, v, listSub]);

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

  const asked = useRef("");
  const oldestHit = list.oldestHit;
  const anchor = useMemo(() => {
    const i =
      oldestHit === null ? -1 : items.findIndex((it) => it.type === "row" && it.key === oldestHit);
    return i < 0 ? items.length - 1 : i;
  }, [items, oldestHit]);
  useEffect(() => {
    const tail = virtual.at(-1);
    if (!list.more || !list.sub || !tail) return;
    const at = `${list.sub}:${oldestHit ?? ""}:${items.length}`;
    if (tail.index >= anchor - 14 && asked.current !== at) {
      asked.current = at;
      act({ type: "moreResults", sub: list.sub });
    }
  }, [virtual, anchor, oldestHit, items.length, list.more, list.sub]);

  const onClick = useCallback(
    (k: string) => {
      if (isDrafts) {
        setHighlight(k);
        resumeDraftById(k);
        return;
      }
      openConversation(Number(k));
    },
    [isDrafts],
  );

  const onHover = useCallback((k: string, e: React.MouseEvent) => {
    if (e.clientX === pointer.x && e.clientY === pointer.y) return;
    const c = useChrome.getState();
    if (c.current()?.highlight === k) return;
    hovered.current = k;
    setHighlight(k);
  }, []);

  const selected = useMemo(() => new Set(selection), [selection]);
  const terms = list.terms;

  if (items.length === 0) {
    const search = cur?.where.kind === "search";
    const needsGmail = search && list.needsGmail && !online;
    return (
      <div className="flex flex-1 flex-col">
        {search && list.searching && (
          <div className="px-4 py-3 text-row text-text-3">Searching Gmail…</div>
        )}
        {needsGmail ? (
          <div className="px-4 py-3 text-row text-text-3">
            This search needs Gmail. Results show when it can be reached.
          </div>
        ) : (
          <Empty inbox={isInbox} loaded={list.loaded && !list.searching} />
        )}
      </div>
    );
  }

  return (
    <div
      ref={parent}
      data-list-container
      className="relative min-h-0 flex-1 overflow-y-auto"
      style={{ contain: "strict" }}
    >
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
          if (!it || it.type !== "row") return null;
          return (
            <div
              key={vi.key}
              role="none"
              style={{
                position: "absolute",
                top: 0,
                left: 0,
                right: 0,
                transform: `translateY(${vi.start}px)`,
              }}
            >
              <Row
                row={rows[it.key]}
                rowKey={it.key}
                highlighted={it.key === highlight}
                selected={selected.has(it.key)}
                onClick={onClick}
                onHover={onHover}
                terms={terms}
              />
            </div>
          );
        })}
      </div>
      {virtual.map((vi) => {
        const it = items[vi.index];
        if (!it || it.type !== "header") return null;
        return (
          <div
            key={vi.key}
            style={{
              position: "absolute",
              top: 0,
              left: 0,
              right: 0,
              transform: `translateY(${vi.start}px)`,
              pointerEvents: "none",
            }}
          >
            <SplitHeader split={it.split} name={it.name} count={it.count} />
          </div>
        );
      })}
    </div>
  );
}
