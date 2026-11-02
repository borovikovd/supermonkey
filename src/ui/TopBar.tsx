import { ArrowLeft, ChevronDown, ChevronUp, Search } from "lucide-react";
import { useEffect, useMemo, useRef } from "react";

import { goTo } from "../commands";
import { closeComposerOnScreen } from "../compose/start";
import { appHooks } from "../keyboard/refs";
import { useChrome, viewTitle } from "../store/chrome";
import { listKey, useAppState, useCurrent, useListView, useSummary } from "../store/hooks";

export const countLabel = (n: number) => (n > 999 ? "999+" : String(n));

/** Closes search and restores the view it replaced. */
function closeSearch() {
  const c = useChrome.getState();
  const before = c.beforeSearch ?? { kind: "view" as const, view: { kind: "inbox" as const } };
  c.set({ search: null, beforeSearch: null });
  goTo(before);
}

/** Split names with counts for the Inbox header and section headers. */
export function useSplitCounts(account: number | null) {
  const { cur } = useCurrent();
  const summary = useSummary(account);
  const key = account !== null && cur ? listKey(account, cur.where) : null;
  const list = useListView(key);
  const forced = useChrome((s) => s.forcedSplit);
  return useMemo(() => {
    const counts = new Map<number, number>();
    for (const e of list.ids)
      if (e.split !== null) counts.set(e.split, (counts.get(e.split) ?? 0) + 1);
    const splits = (summary?.splits ?? [])
      .filter((s) => s.enabled || s.kind === "other")
      .map((s) => ({
        id: s.id,
        name: s.name,
        hideWhenEmpty: s.hideWhenEmpty,
        count: counts.get(s.id) ?? 0,
      }))
      .filter((s) => s.count > 0 || !s.hideWhenEmpty || s.id === forced);
    return splits;
  }, [list.ids, summary, forced]);
}

function SearchField() {
  const search = useChrome((s) => s.search);
  const ref = useRef<HTMLInputElement>(null);
  const online = useAppState()?.online ?? true;

  useEffect(() => {
    appHooks.focusSearch = () => {
      const c = useChrome.getState();
      const cur = c.current();
      if (!c.search)
        c.set({
          search: { open: true, query: "", focus: "field" },
          beforeSearch: cur?.where ?? null,
        });
      else c.set({ search: { ...c.search, focus: "field" } });
      requestAnimationFrame(() => ref.current?.focus());
    };
  }, []);

  if (!search) return null;
  return (
    <div className="flex items-center gap-2" data-tauri-drag-region="false">
      <Search size={14} strokeWidth={1.5} className="text-text-3" />
      <input
        ref={ref}
        autoFocus
        className="w-72 bg-transparent text-row outline-none placeholder:text-text-3"
        placeholder="Search"
        value={search.query}
        onChange={(e) => {
          const query = e.target.value;
          const c = useChrome.getState();
          c.set({ search: { open: true, query, focus: "field" } });
          if (query.trim()) {
            c.patchCurrent({
              where: { kind: "search", query },
              highlight: null,
              screen: { kind: "list" },
            });
          } else if (c.beforeSearch) {
            c.patchCurrent({ where: c.beforeSearch, highlight: null });
          }
        }}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.preventDefault();
            e.stopPropagation();
            ref.current?.blur();
            closeSearch();
          } else if (e.key === "Enter" || e.key === "ArrowDown") {
            e.preventDefault();
            ref.current?.blur();
            const c = useChrome.getState();
            c.set({ search: { ...search, focus: "results" } });
          }
        }}
      />
      {!online && <span className="text-hint text-text-3">Results from this Mac</span>}
    </div>
  );
}

export function TopBar() {
  const { account, cur } = useCurrent();
  const summary = useSummary(account);
  const splits = useSplitCounts(account);
  const highlightSplit = useHighlightSplit();
  const where = cur?.where;
  const searchOpen = useChrome((s) => s.search !== null);

  let title: React.ReactNode = null;
  if (where?.kind === "view" && where.view.kind === "inbox") {
    title = (
      <div className="flex items-center gap-1 text-row">
        {splits.map((s, i) => (
          <span key={s.id} className="flex items-center gap-1">
            {i > 0 && <span className="text-text-3">·</span>}
            <button
              className={s.id === highlightSplit ? "font-semibold text-text" : "text-text-2"}
              onClick={() =>
                document.querySelector(`[data-split="${s.id}"]`)?.scrollIntoView({ block: "start" })
              }
            >
              {s.name}
            </button>
            {s.count > 0 && <span className="tabular text-text-3">{countLabel(s.count)}</span>}
          </span>
        ))}
      </div>
    );
  } else if (where?.kind === "view") {
    const label = where.view.kind === "label" ? where.view.label : null;
    const name = label ? summary?.labels.find((l) => l.id === label)?.name : undefined;
    title = <span className="text-row font-semibold">{viewTitle(where.view, name)}</span>;
  }

  const screen = cur?.screen.kind ?? "list";
  if (screen !== "list" && !searchOpen) {
    // Conversation and compose: back to the list, and previous / next conversation.
    return (
      // Overlaid on the panel so the subject sits on the same row, as in Superhuman.
      <div className="pointer-events-none absolute inset-x-0 top-0 z-10 flex h-12 items-center gap-2 px-4 [&>button]:pointer-events-auto">
        <IconButton
          title="Back (Esc)"
          onClick={() => (screen === "compose" ? closeComposerOnScreen() : press("Escape"))}
        >
          <ArrowLeft size={16} strokeWidth={1.5} />
        </IconButton>
        <div className="flex-1" />
        {screen === "conversation" && (
          <>
            <IconButton title="Previous conversation (K)" onClick={() => press("k")}>
              <ChevronUp size={16} strokeWidth={1.5} />
            </IconButton>
            <IconButton title="Next conversation (J)" onClick={() => press("j")}>
              <ChevronDown size={16} strokeWidth={1.5} />
            </IconButton>
          </>
        )}
      </div>
    );
  }

  return (
    <div
      data-tauri-drag-region
      className="flex h-12 shrink-0 items-center gap-6 border-b border-border-2 px-5"
    >
      {!searchOpen && title}
      <div className="flex-1" data-tauri-drag-region />
      <SearchField />
      {!searchOpen && (
        <IconButton title="Search (/)" onClick={() => appHooks.focusSearch()}>
          <Search size={16} strokeWidth={1.5} />
        </IconButton>
      )}
    </div>
  );
}

/** Sends a key to the shortcut handler, so buttons and keys share one path. */
function press(k: string) {
  window.dispatchEvent(new KeyboardEvent("keydown", { key: k }));
}

function IconButton({
  title,
  onClick,
  children,
}: {
  title: string;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      title={title}
      onClick={onClick}
      className="flex h-7 w-7 items-center justify-center rounded-control text-text-2 hover:bg-fill hover:text-text"
    >
      {children}
    </button>
  );
}

/** The split holding the highlight. */
function useHighlightSplit(): number | null {
  const { account, cur } = useCurrent();
  const key = account !== null && cur ? listKey(account, cur.where) : null;
  const list = useListView(key);
  const h = cur?.highlight;
  return list.ids.find((e) => e.key === h)?.split ?? list.ids[0]?.split ?? null;
}
