// List and conversation navigation: j/k, x, Shift+J/K, select all, Tab
// between splits, n/p/o inside a conversation.

import type { ConversationData } from "../ipc/types";
import { focusOf, formerNeighbor, highlight, openConversation, visibleSplits } from "../navigation";
import { useChrome } from "../store/chrome";
import { currentListView, visibleIds } from "../store/current";
import { blocks } from "../ui/folds";

const chrome = () => useChrome.getState();

export function moveHighlight(delta: number) {
  const c = chrome();
  const cur = c.current();
  if (!cur) return;
  const ids = visibleIds();
  if (ids.length === 0) return;
  const focus = focusOf();
  let at = focus === null ? -1 : ids.indexOf(focus);
  if (at < 0 && focus !== null) {
    // The focused row left: step to its old neighbor.
    const from = formerNeighbor(focus, ids, delta > 0);
    if (from != null) at = ids.indexOf(from) - delta;
  }
  if (at < 0) at = delta > 0 ? -1 : ids.length;
  const next = Math.min(ids.length - 1, Math.max(0, at + delta));
  const key = ids[next];
  if (key === undefined) return;
  if (cur.screen.kind === "conversation") {
    if (key !== focus) openConversation(Number(key));
    return;
  }
  // Plain j/k replace a selection with the one conversation they move to.
  highlight(key);
  if (c.selection.length > 0) c.setSelection([]);
}

/** Adds `x` if missing, else removes it. */
function toggled<T>(xs: readonly T[], x: T): T[] {
  const set = new Set(xs);
  if (!set.delete(x)) set.add(x);
  return [...set];
}

export function toggleSelect() {
  const c = chrome();
  const h = c.current()?.highlight;
  if (!h) return;
  c.setSelection(toggled(c.selection, h));
}

export function extendSelection(delta: number) {
  const cur = chrome().current();
  if (!cur || cur.screen.kind !== "list") return;
  const h = cur.highlight;
  const sel = new Set(chrome().selection);
  if (h) sel.add(h);
  moveHighlight(delta);
  const nh = chrome().current()?.highlight;
  if (nh) sel.add(nh);
  chrome().setSelection([...sel]);
}

/**
 * The rows Select All covers, as in Superhuman: in a split Inbox the split that
 * holds the highlight, otherwise the whole list.
 */
function selectionScope(highlighted: string | null): string[] {
  const ids = visibleIds();
  const entries = currentListView()?.ids ?? [];
  const split = entries.find((e) => e.key === highlighted)?.split ?? null;
  if (split === null) return ids;
  const inSplit = new Set(entries.filter((e) => e.split === split).map((e) => e.key));
  return ids.filter((k) => inSplit.has(k));
}

/** `Cmd+Shift+A` selects every row in scope; `Cmd+A` the highlighted row and those below it. */
export function selectAll(fromHere: boolean) {
  const cur = chrome().current();
  if (!cur || cur.screen.kind !== "list") return;
  const scope = selectionScope(cur.highlight);
  const at = fromHere && cur.highlight !== null ? scope.indexOf(cur.highlight) : 0;
  if (at < 0) return;
  chrome().setSelection(scope.slice(at));
}

/** Tab / Shift+Tab: first conversation of the next / previous visible split. */
export function jumpSplit(delta: number) {
  const cur = chrome().current();
  if (!cur || cur.screen.kind !== "list") return;
  const splits = visibleSplits();
  if (splits.length === 0) return;
  const entries = currentListView()?.ids ?? [];
  const hSplit = entries.find((e) => e.key === cur.highlight)?.split ?? null;
  const at = splits.findIndex((s) => s.split === hSplit);
  // Outside any split, forward goes to the first one.
  const target = splits[at < 0 && delta > 0 ? 0 : at + delta];
  if (target) highlight(target.first);
}

// ------------------------------------------------------------ conversation

/** Messages `n`/`p` select; drafts and the Sending row are not in `messages`. */
export function selectable(data: ConversationData): number[] {
  return data.messages.map((m) => m.id);
}

export function selectMessage(data: ConversationData, delta: number) {
  const c = chrome();
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
  c.selectMessage(next, fold?.type === "fold" ? fold.ids : undefined);
  requestAnimationFrame(() =>
    document.querySelector(`[data-message="${next}"]`)?.scrollIntoView({ block: "nearest" }),
  );
}

export function toggleExpand(data: ConversationData) {
  const c = chrome();
  const id = c.selectedMessage ?? data.messages.at(-1)?.id;
  if (id === undefined) return;
  c.toggleExpanded(id, false);
}

export function toggleExpandAll() {
  chrome().toggleExpandAll();
}
