import { ArrowLeft, ChevronDown, ChevronUp, Search, X } from "lucide-react";

import { composerKind, useComposeState } from "../compose/session";
import { dom } from "../keyboard/refs";
import { useCommand, type CommandId } from "../keyboard/table";
import { editSearch, highlight as moveHighlightTo, visibleSplits } from "../navigation";
import { useChrome, viewTitle } from "../store/chrome";
import { useAppState, useCurrent, useSummary } from "../store/hooks";
import { useCurrentList, useSplitCounts } from "./current-list";
import { countLabel } from "./format";

function SearchField() {
  const search = useChrome((s) => s.search);
  const online = useAppState()?.online ?? true;

  if (!search) return null;
  // As in Superhuman: the query takes the header, in line with the senders
  // below, with the accent edge while it has focus and a close button.
  return (
    <div className="flex min-w-0 flex-1 items-center gap-3" data-tauri-drag-region="false">
      {search.focus === "field" && (
        <span className="absolute top-0 bottom-0 left-0 w-0.75 bg-accent" />
      )}
      <input
        ref={(el) => {
          dom.searchInput = el;
        }}
        // Focus moves into search because the user just pressed `/`.
        // oxlint-disable-next-line jsx-a11y/no-autofocus
        autoFocus
        aria-label="Search"
        className="ml-3.25 min-w-0 flex-1 bg-transparent text-search text-text outline-none placeholder:text-text-3"
        placeholder="Search"
        value={search.query}
        onChange={(e) => editSearch(e.target.value)}
        // Its keys (Enter, ↓, Esc) are the search layer of the dispatcher, which
        // applies while the field has focus.
        onFocus={() => useChrome.getState().setSearchFocus("field")}
        onBlur={() => useChrome.getState().setSearchFocus("results")}
      />
      {!online && <span className="shrink-0 text-hint text-text-3">Results from this Mac</span>}
      <IconButton command="search-close">
        <X size={16} strokeWidth={1.5} />
      </IconButton>
    </div>
  );
}

/** The Inbox title: its splits with counts; a click scrolls to that split. */
function SplitTitles() {
  const splits = useSplitCounts();
  const list = useCurrentList();
  const highlight = useCurrent().cur?.highlight;
  const highlightSplit =
    list.ids.find((e) => e.key === highlight)?.split ?? list.ids[0]?.split ?? null;
  return (
    <div role="toolbar" aria-label="Splits" className="flex items-center gap-1 text-row">
      {splits.map((s, i) => (
        <span key={s.id} className="flex items-center gap-1">
          {i > 0 && <span className="text-text-3">·</span>}
          <button
            aria-current={s.id === highlightSplit || undefined}
            className={s.id === highlightSplit ? "font-semibold text-text" : "text-text-2"}
            // Like Tab, the highlight moves to the split's first row; the list
            // scrolls its header to the top (through the virtual list, which
            // may not have rendered it).
            onClick={() => {
              const first = visibleSplits().find((v) => v.split === s.id)?.first;
              if (!first || useChrome.getState().current()?.highlight === first) return;
              moveHighlightTo(first, { headerToTop: true });
            }}
          >
            {s.name}
          </button>
          {s.count > 0 && (
            <span className="tabular text-hint font-medium text-text-3">{countLabel(s.count)}</span>
          )}
        </span>
      ))}
    </div>
  );
}

function ViewTitle() {
  const { account, cur } = useCurrent();
  const summary = useSummary(account);
  const where = cur?.where;
  if (where?.kind !== "view") return null;
  if (where.view.kind === "inbox") return <SplitTitles />;
  const label = where.view.kind === "label" ? where.view.label : null;
  const name = label ? summary?.labels.find((l) => l.id === label)?.name : undefined;
  return <span className="text-row font-semibold">{viewTitle(where.view, name)}</span>;
}

export function TopBar() {
  const { cur } = useCurrent();
  const searchOpen = useChrome((s) => s.search !== null);
  const composing = useComposeState(composerKind) === "screen";

  const screen = composing ? "compose" : (cur?.screen.kind ?? "list");
  if (screen !== "list" && !searchOpen) {
    // Conversation and compose: back to the list, and previous / next conversation.
    return (
      // Overlaid on the panel so the subject sits on the same row; round floating
      // buttons as in Superhuman.
      <div className="pointer-events-none absolute top-0 left-0 z-10 flex h-12 items-center gap-2 px-4 [&>*]:pointer-events-auto">
        <div className="flex h-8 items-center rounded-full bg-bg px-0.5 shadow-card">
          <IconButton round command="back">
            <ArrowLeft size={16} strokeWidth={1.5} />
          </IconButton>
        </div>
        {screen === "conversation" && (
          <div className="flex h-8 items-center rounded-full bg-bg px-1 shadow-card">
            <IconButton round command="prev">
              <ChevronUp size={16} strokeWidth={1.5} />
            </IconButton>
            <IconButton round command="next">
              <ChevronDown size={16} strokeWidth={1.5} />
            </IconButton>
          </div>
        )}
      </div>
    );
  }

  return (
    <div
      data-tauri-drag-region
      className="relative flex h-12 shrink-0 items-center gap-6 border-b border-border-2 px-5"
    >
      {searchOpen ? (
        <SearchField />
      ) : (
        <>
          <ViewTitle />
          <div className="flex-1" data-tauri-drag-region />
          <IconButton command="search">
            <Search size={16} strokeWidth={1.5} />
          </IconButton>
        </>
      )}
    </div>
  );
}

/** A button for a command: its title from the table, disabled where it does nothing. */
function IconButton({
  command,
  round = false,
  children,
}: {
  command: CommandId;
  round?: boolean;
  children: React.ReactNode;
}) {
  const { title, enabled, run } = useCommand(command);
  return (
    <button
      title={title}
      disabled={!enabled}
      onClick={run}
      className={`flex h-7 w-7 items-center justify-center text-text-2 hover:bg-fill hover:text-text disabled:opacity-40 disabled:hover:bg-transparent ${round ? "rounded-full" : "rounded-md"}`}
    >
      {children}
    </button>
  );
}
