// List and conversation navigation: j/k, x, Shift+J/K, Tab between splits,
// n/p/o inside a conversation.

import {
  closeThread,
  currentListView,
  currentView,
  goTo,
  markReadOnOpenWhenLoaded,
  openThread,
  visibleIds,
} from "../commands";
import type { ConversationData } from "../ipc/types";
import { useChrome } from "../store/chrome";
import { useMirror } from "../store/mirror";
import { blocks } from "../ui/folds";

/** Adds `x` if missing, else removes it. */
function toggled<T>(xs: readonly T[], x: T): T[] {
  const set = new Set(xs);
  if (!set.delete(x)) set.add(x);
  return [...set];
}

/** The focused row and the list it was last seen in, so a row that leaves keeps its place. */
let lastSeen: { focus: string; ids: string[] } | null = null;

function focusOf(): string | null {
  const cur = useChrome.getState().current();
  if (!cur) return null;
  return cur.screen.kind === "conversation" ? String(cur.screen.thread) : cur.highlight;
}

/** From where the focus used to be: the next row still shown, or the previous one. */
function neighbor(focus: string, ids: string[], forward: boolean): string | null {
  if (lastSeen?.focus !== focus) return null;
  const shown = new Set(ids);
  const at = lastSeen.ids.indexOf(focus);
  const after = lastSeen.ids.slice(at + 1).find((k) => shown.has(k));
  const before = lastSeen.ids.slice(0, Math.max(at, 0)).findLast((k) => shown.has(k));
  return (forward ? (after ?? before) : (before ?? after)) ?? null;
}

/** Re-anchors the focus, or moves the highlight off a row that left the list. */
function followFocus() {
  const focus = focusOf();
  if (focus === null) return;
  const ids = visibleIds();
  if (ids.includes(focus)) {
    lastSeen = { focus, ids };
    return;
  }
  if (useChrome.getState().current()?.screen.kind !== "list") return;
  const next = neighbor(focus, ids, true);
  if (next !== null) useChrome.getState().patchCurrent({ highlight: next });
}

/**
 * When a patch from Rust removes the highlighted row, the highlight moves as
 * after an action. An open conversation whose row left stays open; `j` and
 * `k` then move from its old place (SPEC Keyboard).
 */
export function trackFocus() {
  useMirror.subscribe(followFocus);
  useChrome.subscribe(followFocus);
}

export function moveHighlight(delta: number) {
  const c = useChrome.getState();
  const cur = c.current();
  if (!cur) return;
  const ids = visibleIds();
  if (ids.length === 0) return;
  const focus = focusOf();
  let at = focus === null ? -1 : ids.indexOf(focus);
  if (at < 0 && focus !== null) {
    // The focused row left: step to its old neighbor.
    const from = neighbor(focus, ids, delta > 0);
    if (from !== null) at = ids.indexOf(from) - delta;
  }
  if (at < 0) at = delta > 0 ? -1 : ids.length;
  const next = Math.min(ids.length - 1, Math.max(0, at + delta));
  const key = ids[next];
  if (key === undefined) return;
  if (cur.screen.kind === "conversation") {
    if (key !== focus) {
      openThread(Number(key));
      markReadOnOpenWhenLoaded(Number(key));
    }
    return;
  }
  // Plain j/k replace a selection with the one conversation they move to.
  c.patchCurrent({ highlight: key });
  if (c.selection.length > 0) c.set({ selection: [] });
}

export function toggleSelect() {
  const c = useChrome.getState();
  const h = c.current()?.highlight;
  if (!h) return;
  c.set({ selection: toggled(c.selection, h) });
}

export function extendSelection(delta: number) {
  const c = useChrome.getState();
  const cur = c.current();
  if (!cur || cur.screen.kind !== "list") return;
  const h = cur.highlight;
  const sel = new Set(c.selection);
  if (h) sel.add(h);
  moveHighlight(delta);
  const nh = useChrome.getState().current()?.highlight;
  if (nh) sel.add(nh);
  useChrome.getState().set({ selection: [...sel] });
}

/**
 * The rows Select All covers, as in Superhuman: in a split Inbox the split that
 * holds the highlight, otherwise the whole list.
 */
function selectionScope(highlight: string | null): string[] {
  const ids = visibleIds();
  const entries = currentListView()?.ids ?? [];
  const split = entries.find((e) => e.key === highlight)?.split ?? null;
  if (split === null) return ids;
  const inSplit = new Set(entries.filter((e) => e.split === split).map((e) => e.key));
  return ids.filter((k) => inSplit.has(k));
}

/** `Cmd+Shift+A` selects every row in scope; `Cmd+A` the highlighted row and those below it. */
export function selectAll(fromHere: boolean) {
  const c = useChrome.getState();
  const cur = c.current();
  if (!cur || cur.screen.kind !== "list") return;
  const scope = selectionScope(cur.highlight);
  const at = fromHere && cur.highlight !== null ? scope.indexOf(cur.highlight) : 0;
  if (at < 0) return;
  c.set({ selection: scope.slice(at) });
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

/** Tab / Shift+Tab: first conversation of the next / previous visible split. */
export function jumpSplit(delta: number) {
  const c = useChrome.getState();
  const cur = c.current();
  if (!cur || cur.screen.kind !== "list") return;
  const splits = visibleSplits();
  if (splits.length === 0) return;
  const entries = currentListView()?.ids ?? [];
  const hSplit = entries.find((e) => e.key === cur.highlight)?.split ?? null;
  const at = splits.findIndex((s) => s.split === hSplit);
  // Outside any split, forward goes to the first one.
  const target = splits[at < 0 && delta > 0 ? 0 : at + delta];
  if (target) c.patchCurrent({ highlight: target.first });
}

/** `g i` / `g o`: Inbox on the Important or Other split. */
export function goSplit(kind: "important" | "other", splitIdOf: (kind: string) => number | null) {
  goTo({ kind: "view", view: { kind: "inbox" } });
  const target = splitIdOf(kind);
  // A hidden empty split shows its header after this jump.
  useChrome.getState().set({ forcedSplit: target });
  const highlight = () => {
    const split = visibleSplits().find((s) => s.split === target);
    if (split) useChrome.getState().patchCurrent({ highlight: split.first });
    return split !== undefined;
  };
  stopSplitWait?.();
  if (highlight()) return;
  // The list loads asynchronously: highlight once rows for the split arrive,
  // however long that takes, unless the Inbox is left first.
  const { account } = useChrome.getState();
  const stop = () => {
    unsubMirror();
    unsubChrome();
    if (stopSplitWait === stop) stopSplitWait = null;
  };
  const unsubMirror = useMirror.subscribe(() => {
    if (highlight()) stop();
  });
  const unsubChrome = useChrome.subscribe((s) => {
    if (s.account !== account || currentView()?.kind !== "inbox") stop();
  });
  stopSplitWait = stop;
}

/** Ends the wait of the last `g i` / `g o` for its split's rows, if one is waiting. */
let stopSplitWait: (() => void) | null = null;

// ------------------------------------------------------------ conversation

/** Messages `n`/`p` select; drafts and the Sending row are not in `messages`. */
export function selectable(data: ConversationData): number[] {
  return data.messages.map((m) => m.id);
}

export function selectMessage(data: ConversationData, delta: number) {
  const c = useChrome.getState();
  const ids = selectable(data);
  const last = ids.length - 1;
  const at = c.selectedMessage === null ? -1 : ids.indexOf(c.selectedMessage);
  const next = ids[Math.min(last, Math.max(0, (at < 0 ? last : at) + delta))];
  if (next === undefined) return;
  // Selecting a message inside a fold opens the whole fold, as a click on it does.
  const expanded = new Set(c.expandAll ? ids : c.expanded);
  const unfolded = new Set(
    c.selectedMessage === null ? c.unfolded : [...c.unfolded, c.selectedMessage],
  );
  const fold = blocks(data.messages, expanded, unfolded).find(
    (b) => b.type === "fold" && b.ids.includes(next),
  );
  c.set({
    selectedMessage: next,
    unfolded: fold?.type === "fold" ? [...c.unfolded, ...fold.ids] : c.unfolded,
  });
  requestAnimationFrame(() =>
    document.querySelector(`[data-message="${next}"]`)?.scrollIntoView({ block: "nearest" }),
  );
}

export function toggleExpand(data: ConversationData) {
  const c = useChrome.getState();
  const id = c.selectedMessage ?? data.messages.at(-1)?.id;
  if (id === undefined) return;
  c.set({ expanded: toggled(c.expanded, id) });
}

export function toggleExpandAll() {
  const c = useChrome.getState();
  c.set({ expandAll: !c.expandAll });
}

export { closeThread };
