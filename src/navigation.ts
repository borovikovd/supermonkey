// Screen transitions (SPEC Window, Keyboard, Search, Switching): what the UI
// calls to change what is on screen. Each applies its rules here, once:
// leaving a screen closes its composer as a second Esc would, opening a
// conversation marks it read once loaded, and the highlight moves on when its
// row leaves the list.

import { closeComposerOnScreen } from "./compose/session";
import { act } from "./ipc/bridge";
import type { View } from "./ipc/types";
import { dom, listScroll } from "./keyboard/refs";
import { useChrome, type PaletteMode, type Sheet } from "./store/chrome";
import {
  currentListLoaded,
  currentView,
  listShowsTrash,
  rowOf,
  splitIdOf,
  visibleIds,
  currentListView,
} from "./store/current";
import { useMirror } from "./store/mirror";
import { appState } from "./store/mirror-core";

const chrome = () => useChrome.getState();

// ------------------------------------------------------------ views

/** Shows a view's list, with its top row highlighted once it has rows. */
export function goTo(view: View) {
  if (chrome().account === null) return;
  closeComposerOnScreen();
  chrome().showList({ kind: "view", view });
}

/** Visible splits of the Inbox list in order, with their first conversation. */
export function visibleSplits(): { split: number; first: string }[] {
  const list = currentListView();
  if (!list || currentView()?.kind !== "inbox") return [];
  const out: { split: number; first: string }[] = [];
  for (const e of list.ids) {
    if (e.split !== null && !out.some((s) => s.split === e.split))
      out.push({ split: e.split, first: e.key });
  }
  return out;
}

/** Ends the wait of the last `g i` / `g o` for its split's rows, if one is waiting. */
let stopSplitWait: (() => void) | null = null;

/** `g i` / `g o`: Inbox on the Important or Other split; a hidden empty one shows its header. */
export function goToSplit(kind: "important" | "other") {
  goTo({ kind: "inbox" });
  const target = splitIdOf(kind);
  chrome().forceSplit(target);
  const highlightSplit = () => {
    const split = visibleSplits().find((s) => s.split === target);
    if (split) chrome().setHighlight(split.first);
    return split !== undefined;
  };
  stopSplitWait?.();
  if (highlightSplit()) return;
  // The list loads asynchronously: highlight once rows for the split arrive,
  // however long that takes, unless the Inbox is left first.
  const { account } = chrome();
  const stop = () => {
    unsubMirror();
    unsubChrome();
    if (stopSplitWait === stop) stopSplitWait = null;
  };
  const unsubMirror = useMirror.subscribe(() => {
    if (highlightSplit()) stop();
  });
  const unsubChrome = useChrome.subscribe((s) => {
    if (s.account !== account || currentView()?.kind !== "inbox") stop();
  });
  stopSplitWait = stop;
}

/** Moves the highlight; `headerToTop` also scrolls its split header to the top. */
export function highlight(key: string, opts: { headerToTop?: boolean } = {}) {
  if (opts.headerToTop) listScroll.headerToTop = true;
  chrome().setHighlight(key);
}

// ------------------------------------------------------------ conversations

/** Marks the open thread read; not an undo entry. */
export function markReadOnOpen(thread: number) {
  const { account } = chrome();
  const row = rowOf(String(thread));
  if (account === null || (row && !row.unread)) return;
  act({
    type: "modify",
    account,
    kind: "read",
    threads: [thread],
    add: [],
    remove: ["UNREAD"],
    snooze: null,
    undoable: false,
  });
}

/**
 * Marking read on open is an action the UI sends after the conversation's
 * first load, or at once if it was prefetched.
 */
function markReadWhenLoaded(thread: number) {
  const loaded = () =>
    Object.values(useMirror.getState().subs).some(
      (s) => s.type === "conversation" && s.key.thread === thread && s.data,
    );
  if (loaded()) {
    markReadOnOpen(thread);
    return;
  }
  // Only while it stays open: data that arrives later, say from the prefetch
  // after the user left, was never on screen.
  const onScreen = () => {
    const screen = chrome().current()?.screen;
    return screen?.kind === "conversation" && screen.thread === thread;
  };
  const stop = () => {
    unsubMirror();
    unsubChrome();
  };
  const unsubMirror = useMirror.subscribe(() => {
    if (!loaded()) return;
    stop();
    markReadOnOpen(thread);
  });
  const unsubChrome = useChrome.subscribe(() => {
    if (!onScreen()) stop();
  });
}

/**
 * Opens a conversation from the list (SPEC Reading). Its row, `row`, takes
 * the highlight: the thread's own row, except in Drafts, whose rows are
 * drafts, where it is the draft's row and stays the highlighted one by default.
 */
export function openConversation(thread: number, row?: string) {
  closeComposerOnScreen();
  const view = currentView();
  const from =
    row ?? (view?.kind === "drafts" ? chrome().current()?.highlight : null) ?? String(thread);
  chrome().showConversation(thread, listShowsTrash(), from);
  markReadWhenLoaded(thread);
}

/** Back to the list, keeping the highlight. */
export function closeConversation() {
  closeComposerOnScreen();
  chrome().leaveConversation();
}

// ------------------------------------------------------------ the focus when rows leave

/**
 * Per account, the focused row and the list it was last seen in, so a row
 * that leaves keeps its place, even while another account is on screen.
 */
const lastSeen = new Map<number, { focus: string; ids: string[] }>();

/**
 * The highlighted row; in an open conversation, the row it was opened from
 * (a draft's, in Drafts), which holds the highlight.
 */
export function focusOf(): string | null {
  const cur = chrome().current();
  if (!cur) return null;
  return cur.screen.kind === "conversation"
    ? (cur.highlight ?? String(cur.screen.thread))
    : cur.highlight;
}

/**
 * From where the focus last sat in the list: the next row still shown, or
 * the previous one; null when none is left; undefined when the focus was
 * never seen in this list.
 */
export function formerNeighbor(
  focus: string,
  ids: string[],
  forward: boolean,
): string | null | undefined {
  const { account } = chrome();
  const seen = account === null ? undefined : lastSeen.get(account);
  if (seen?.focus !== focus) return undefined;
  const shown = new Set(ids);
  const at = seen.ids.indexOf(focus);
  const after = seen.ids.slice(at + 1).find((k) => shown.has(k));
  const before = seen.ids.slice(0, Math.max(at, 0)).findLast((k) => shown.has(k));
  return (forward ? (after ?? before) : (before ?? after)) ?? null;
}

/**
 * Keeps the highlight on a shown row. A row that leaves, by the user's action
 * or a patch from Rust, passes the highlight to its next row, or the previous
 * one; with none left the list has no highlight, and the top row takes it
 * once one arrives, as on a first visit. An open conversation whose row left
 * stays open (SPEC Keyboard).
 */
function followFocus() {
  const c = chrome();
  const cur = c.current();
  if (c.account === null || !cur) return;
  const ids = visibleIds();
  const focus = focusOf();
  if (focus === null) {
    const top = ids[0];
    if (cur.screen.kind === "list" && top !== undefined) c.setHighlight(top);
    return;
  }
  if (ids.includes(focus)) {
    lastSeen.set(c.account, { focus, ids });
    return;
  }
  // Before its first load a list has no rows yet, not no rows.
  if (cur.screen.kind !== "list" || !currentListLoaded()) return;
  c.setHighlight(formerNeighbor(focus, ids, true) ?? null);
}

/** Installs the highlight rule above; call once at startup. */
export function trackFocus() {
  useMirror.subscribe(followFocus);
  useChrome.subscribe(followFocus);
}

/**
 * After the user's action on `acted` took the open conversation out of the
 * list: the next conversation opens, or the previous one if it was last, or
 * the list when none remain. On the list, the highlight has already moved.
 */
export function moveOn(acted: number[]) {
  const screen = chrome().current()?.screen;
  if (screen?.kind !== "conversation" || !acted.includes(screen.thread)) return;
  const focus = String(screen.thread);
  const ids = visibleIds();
  if (ids.includes(focus)) return;
  const next = formerNeighbor(focus, ids, true);
  // A conversation opened from elsewhere (a draft in Drafts) has no row to move from.
  if (next === undefined) return;
  if (next === null) closeConversation();
  else openConversation(Number(next));
}

// ------------------------------------------------------------ search

/** `/`: search for the account on screen, with focus in the field (SPEC Search). */
export function openSearch() {
  if (chrome().account === null) return;
  chrome().openSearch();
  requestAnimationFrame(() => dom.searchInput?.focus());
}

/** Typing in the search field: results replace the screen, and a composer on it closes. */
export function editSearch(query: string) {
  if (query.trim()) closeComposerOnScreen();
  chrome().setSearchQuery(query);
}

/** `Enter` or `↓` in the field: focus moves to the results. */
export function showSearchResults() {
  dom.searchInput?.blur();
  chrome().setSearchFocus("results");
}

/** Closes search and restores the view and highlight it replaced. */
export function closeSearch() {
  closeComposerOnScreen();
  dom.searchInput?.blur();
  chrome().closeSearch();
}

// ------------------------------------------------------------ accounts

/** Switching accounts closes the palette, search, sheets, and an open composer (SPEC Switching). */
export function switchAccount(account: number) {
  const c = chrome();
  if (c.account === account) return;
  closeComposerOnScreen();
  // An open conversation is not reopened; the list is.
  if (c.account !== null) c.leaveConversation();
  c.setAccount(account);
  c.closeSheet();
  if (c.current()?.screen.kind !== "list") c.leaveConversation();
}

/** `Ctrl+1`…`Ctrl+9`: the account at that place in the rail. */
export function switchAccountAt(index: number) {
  const a = appState(useMirror.getState())?.accounts[index];
  if (a) switchAccount(a.id);
}

/** Keeps an account on screen: the first one, when none is yet or the one on screen was removed. */
export function followAccounts(ids: number[]) {
  const { account } = chrome();
  if (account === null || !ids.includes(account)) chrome().setAccount(ids[0] ?? null);
}

/** A notification click: that account, Inbox, the thread highlighted. */
export function openNotification(account: number, thread: number | null) {
  switchAccount(account);
  goTo({ kind: "inbox" });
  if (thread !== null) chrome().setHighlight(String(thread));
}

// ------------------------------------------------------------ overlays

export const openPalette = (mode: PaletteMode) => chrome().openPalette(mode);

export const closePalette = () => chrome().closePalette();

export const openSheet = (sheet: Exclude<Sheet, null>) => chrome().openSheet(sheet);

export const closeSheet = () => chrome().closeSheet();

/** Split Inbox settings of an account, which comes on screen. */
export function openSplitSettings(account: number) {
  switchAccount(account);
  openSheet("splits");
}
