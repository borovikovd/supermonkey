import { fixedTargets } from "./acting";
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

export function goTo(view: View) {
  if (chrome().account === null) return;
  closeComposerOnScreen();
  chrome().showList({ kind: "view", view });
}

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

function whenMirrorHas<T>(
  find: () => T | undefined,
  left: (s: ReturnType<typeof chrome>) => boolean,
  run: (found: T) => void,
): (() => void) | null {
  const now = find();
  if (now !== undefined) {
    run(now);
    return null;
  }
  const stop = () => {
    unsubMirror();
    unsubChrome();
  };
  const unsubMirror = useMirror.subscribe(() => {
    const found = find();
    if (found === undefined) return;
    stop();
    run(found);
  });
  const unsubChrome = useChrome.subscribe((s) => {
    if (left(s)) stop();
  });
  return stop;
}

let stopSplitWait: (() => void) | null = null;

export function goToSplit(kind: "important" | "other") {
  goTo({ kind: "inbox" });
  const target = splitIdOf(kind);
  chrome().forceSplit(target);
  stopSplitWait?.();
  const { account } = chrome();
  stopSplitWait = whenMirrorHas(
    () => visibleSplits().find((s) => s.split === target),
    (s) => s.account !== account || currentView()?.kind !== "inbox",
    (split) => chrome().setHighlight(split.first),
  );
}

export function highlight(key: string, opts: { headerToTop?: boolean } = {}) {
  if (opts.headerToTop) listScroll.headerToTop = true;
  chrome().setHighlight(key);
}

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

function markReadWhenLoaded(thread: number) {
  const loaded = () =>
    Object.values(useMirror.getState().subs).some(
      (s) => s.type === "conversation" && s.key.thread === thread && s.data,
    );
  const onScreen = () => {
    const screen = chrome().current()?.screen;
    return screen?.kind === "conversation" && screen.thread === thread;
  };
  whenMirrorHas(
    () => (loaded() ? thread : undefined),
    () => !onScreen(),
    markReadOnOpen,
  );
}

export function openConversation(thread: number, row?: string) {
  closeComposerOnScreen();
  const view = currentView();
  const from =
    row ?? (view?.kind === "drafts" ? chrome().current()?.highlight : null) ?? String(thread);
  chrome().showConversation(thread, listShowsTrash(), from);
  markReadWhenLoaded(thread);
}

export function closeConversation() {
  closeComposerOnScreen();
  chrome().leaveConversation();
}

const lastSeen = new Map<number, { focus: string; ids: string[] }>();

export function focusOf(): string | null {
  const cur = chrome().current();
  if (!cur) return null;
  return cur.screen.kind === "conversation"
    ? (cur.highlight ?? String(cur.screen.thread))
    : cur.highlight;
}

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
  if (cur.screen.kind !== "list" || !currentListLoaded()) return;
  c.setHighlight(formerNeighbor(focus, ids, true) ?? null);
}

export function trackFocus() {
  useMirror.subscribe(followFocus);
  useChrome.subscribe(followFocus);
}

export function moveOn(acted: number[]) {
  const screen = chrome().current()?.screen;
  if (screen?.kind !== "conversation" || !acted.includes(screen.thread)) return;
  const focus = String(screen.thread);
  const ids = visibleIds();
  if (ids.includes(focus)) return;
  const next = formerNeighbor(focus, ids, true);
  if (next === undefined) return;
  if (next === null) closeConversation();
  else openConversation(Number(next));
}

export function openSearch() {
  if (chrome().account === null) return;
  chrome().openSearch();
  requestAnimationFrame(() => dom.searchInput?.focus());
}

export function editSearch(query: string) {
  if (query.trim()) closeComposerOnScreen();
  chrome().setSearchQuery(query);
}

export function showSearchResults() {
  dom.searchInput?.blur();
  chrome().setSearchFocus("results");
}

export function closeSearch() {
  closeComposerOnScreen();
  dom.searchInput?.blur();
  chrome().closeSearch();
}

export function switchAccount(account: number) {
  const c = chrome();
  if (c.account === account) return;
  closeComposerOnScreen();
  if (c.account !== null) c.leaveConversation();
  c.setAccount(account);
  c.closeSheet();
  if (c.current()?.screen.kind !== "list") c.leaveConversation();
}

export function switchAccountAt(index: number) {
  const a = appState(useMirror.getState())?.accounts[index];
  if (a) switchAccount(a.id);
}

let rail: number[] = [];

export function followAccounts(ids: number[]) {
  const before = rail;
  rail = ids;
  const { account } = chrome();
  if (account !== null && ids.includes(account)) return;
  closeComposerOnScreen();
  chrome().setAccount(account === null ? (ids[0] ?? null) : successor(before, ids, account));
}

function successor(before: number[], ids: number[], removed: number): number | null {
  const at = before.indexOf(removed);
  if (at < 0) return ids[0] ?? null;
  return before.slice(at + 1).find((id) => ids.includes(id)) ?? ids.at(-1) ?? null;
}

export function openNotification(account: number, thread: number | null) {
  switchAccount(account);
  goTo({ kind: "inbox" });
  if (thread !== null) chrome().setHighlight(String(thread));
}

export const openPalette = (mode: PaletteMode) => {
  const bound = fixedTargets();
  chrome().openPalette(bound && mode.threads === undefined ? { ...mode, threads: bound } : mode);
};

export const closePalette = () => chrome().closePalette();

export const openSheet = (sheet: Exclude<Sheet, null>) => chrome().openSheet(sheet);

export const closeSheet = () => chrome().closeSheet();

export function openSplitSettings(account: number) {
  switchAccount(account);
  openSheet("splits");
}
