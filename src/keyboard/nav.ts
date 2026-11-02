// List and conversation navigation: j/k, x, Shift+J/K, Tab between splits,
// n/p/o inside a conversation.

import {
  closeThread,
  currentListKey,
  currentView,
  goTo,
  markReadOnOpenWhenLoaded,
  openThread,
  visibleIds,
} from "../commands";
import { subIdFor } from "../ipc/bridge";
import type { ConversationData } from "../ipc/types";
import { useChrome } from "../store/chrome";
import { viewList } from "../store/mirror-core";
import { useMirror } from "../store/mirror";

export function moveHighlight(delta: number) {
  const c = useChrome.getState();
  const cur = c.current();
  if (!cur) return;
  const ids = visibleIds();
  if (ids.length === 0) return;
  const focus = cur.screen.kind === "conversation" ? String(cur.screen.thread) : cur.highlight;
  let at = focus === null ? -1 : ids.indexOf(focus);
  if (at < 0) at = delta > 0 ? -1 : ids.length;
  const next = Math.min(ids.length - 1, Math.max(0, at + delta));
  const key = ids[next];
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
  const sel = new Set(c.selection);
  if (sel.has(h)) sel.delete(h);
  else sel.add(h);
  c.set({ selection: [...sel] });
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

/** Visible splits of the Inbox list in order, with their first conversation. */
export function visibleSplits(): { split: number; first: string }[] {
  const key = currentListKey();
  const id = key ? subIdFor(key) : undefined;
  if (!id || currentView()?.kind !== "inbox") return [];
  const out: { split: number; first: string }[] = [];
  for (const e of viewList(useMirror.getState(), id).ids) {
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
  const ids = visibleIds();
  const key = currentListKey();
  const id = key ? subIdFor(key) : undefined;
  const entries = id ? viewList(useMirror.getState(), id).ids : [];
  const hSplit = entries.find((e) => e.key === cur.highlight)?.split ?? null;
  const at = splits.findIndex((s) => s.split === hSplit);
  const next = at + delta;
  const first = splits[0];
  if (at < 0 && delta > 0 && ids.length > 0 && first) {
    c.patchCurrent({ highlight: first.first });
    return;
  }
  const target = splits[next];
  if (target) c.patchCurrent({ highlight: target.first });
}

/** `g i` / `g o`: Inbox on the Important or Other split. */
export function goSplit(kind: "important" | "other", splitIdOf: (kind: string) => number | null) {
  goTo({ kind: "view", view: { kind: "inbox" } });
  const target = splitIdOf(kind);
  // A hidden empty split shows its header after this jump.
  useChrome.getState().set({ forcedSplit: target });
  // The list loads asynchronously; highlight once rows for the split arrive.
  const tryHighlight = (left: number) => {
    const split = visibleSplits().find((s) => s.split === target);
    if (split) useChrome.getState().patchCurrent({ highlight: split.first });
    else if (left > 0) setTimeout(() => tryHighlight(left - 1), 30);
  };
  tryHighlight(10);
}

// ------------------------------------------------------------ conversation

/** Messages that can be selected (no collapsed drafts, no Sending row). */
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
  c.set({ selectedMessage: next, unfolded: [...c.unfolded, next] });
  requestAnimationFrame(() =>
    document.querySelector(`[data-message="${next}"]`)?.scrollIntoView({ block: "nearest" }),
  );
}

export function toggleExpand(data: ConversationData) {
  const c = useChrome.getState();
  const id = c.selectedMessage ?? data.messages[data.messages.length - 1]?.id;
  if (id === undefined) return;
  const ex = new Set(c.expanded);
  if (ex.has(id)) ex.delete(id);
  else ex.add(id);
  c.set({ expanded: [...ex] });
}

export function toggleExpandAll() {
  const c = useChrome.getState();
  c.set({ expandAll: !c.expandAll });
}

export { closeThread };
