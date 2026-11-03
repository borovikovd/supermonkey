// Mail commands shared by the keyboard and the palette. Each resolves its
// targets, sends a resolved action (never a toggle), and moves the highlight
// when rows leave the list (SPEC Keyboard).

import { undoSend } from "./compose/undo-send";
import { act, subIdFor } from "./ipc/bridge";
import type { ModifyAction, ModifyKind, SubKey, View } from "./ipc/types";
import { useChrome, type Where } from "./store/chrome";
import { appHooks } from "./keyboard/refs";
import { listKey } from "./store/hooks";
import {
  accountSummary,
  undoTarget,
  viewLabel,
  viewList,
  type ListView,
} from "./store/mirror-core";
import { useMirror } from "./store/mirror";

export function currentListKey(): SubKey | null {
  const c = useChrome.getState();
  const cur = c.current();
  return c.account === null || !cur ? null : listKey(c.account, cur.where);
}

export function currentView(): View | null {
  const where = useChrome.getState().current()?.where;
  return where?.kind === "view" ? where.view : null;
}

/** The list on screen with pending actions applied, once it is subscribed. */
export function currentListView(): ListView | null {
  const key = currentListKey();
  const id = key ? subIdFor(key) : undefined;
  return id ? viewList(useMirror.getState(), id) : null;
}

export function visibleIds(): string[] {
  return currentListView()?.ids.map((e) => e.key) ?? [];
}

export function rowOf(key: string) {
  return (currentListView()?.rows ?? useMirror.getState().rows)[key];
}

/** Threads an action applies to: the selection, else the open conversation, else the highlight. */
export function targets(): number[] {
  const c = useChrome.getState();
  const cur = c.current();
  if (!cur) return [];
  if (c.selection.length > 0 && cur.screen.kind === "list") {
    return c.selection.map(Number).filter((n) => !Number.isNaN(n));
  }
  if (cur.screen.kind === "conversation") return [cur.screen.thread];
  if (cur.highlight !== null && currentView()?.kind !== "drafts") {
    const n = Number(cur.highlight);
    return Number.isNaN(n) ? [] : [n];
  }
  return [];
}

/** Opens a conversation from the list. */
export function openThread(thread: number) {
  const c = useChrome.getState();
  const view = currentView();
  const trashMode = view?.kind === "trash" || view?.kind === "spam";
  c.patchCurrent({
    screen: { kind: "conversation", thread, trashMode },
    highlight: String(thread),
  });
  c.set({
    selectedMessage: null,
    expanded: [],
    unfolded: [],
    expandAll: false,
    link: null,
    inline: null,
    selection: [],
  });
}

export function closeThread() {
  const c = useChrome.getState();
  c.patchCurrent({ screen: { kind: "list" } });
  c.set({ inline: null, link: null, selectedMessage: null });
}

/**
 * After rows leave the list: on the list the highlight moves to the next row
 * (or the previous one if it was last); in an open conversation the next
 * conversation opens; with none left, the empty list.
 */
function moveAfterRemoval(before: string[], removed: Set<string>) {
  const c = useChrome.getState();
  const cur = c.current();
  if (!cur) return;
  const shown = new Set(visibleIds());
  const focus = cur.screen.kind === "conversation" ? String(cur.screen.thread) : cur.highlight;
  if (focus === null || shown.has(focus) || !removed.has(focus)) return;
  const at = before.indexOf(focus);
  // A conversation opened from elsewhere (a draft in Drafts) has no row to move from.
  if (at < 0) return;
  const isShown = (k: string) => shown.has(k);
  const next = before.slice(at + 1).find(isShown) ?? before.slice(0, at).findLast(isShown) ?? null;
  if (cur.screen.kind === "conversation") {
    if (next === null) closeThread();
    else openThread(Number(next));
  } else {
    c.patchCurrent({ highlight: next });
  }
}

export function modify(
  kind: ModifyKind,
  add: string[],
  remove: string[],
  opts: { snooze?: ModifyAction["snooze"]; threads?: number[]; undoable?: boolean } = {},
) {
  const c = useChrome.getState();
  if (c.account === null) return;
  const threads = opts.threads ?? targets();
  if (threads.length === 0) return;
  const before = visibleIds();
  act({
    type: "modify",
    account: c.account,
    kind,
    threads,
    add,
    remove,
    snooze: opts.snooze ?? null,
    undoable: opts.undoable ?? true,
  });
  if (opts.undoable !== false) c.set({ selection: [] });
  moveAfterRemoval(before, new Set(threads.map(String)));
}

const isSnoozed = (threads: number[]) => threads.some((t) => rowOf(String(t))?.snoozeUntil != null);

export const markDone = () => {
  if (currentView()?.kind === "done") return;
  modify("done", [], ["INBOX"], { snooze: isSnoozed(targets()) ? { kind: "clear" } : null });
};

export const returnToInbox = () => {
  const view = currentView();
  const remove = view?.kind === "trash" ? ["TRASH"] : view?.kind === "spam" ? ["SPAM"] : [];
  modify("returnToInbox", ["INBOX"], remove, {
    snooze: isSnoozed(targets()) ? { kind: "clear" } : null,
  });
};

export const trash = () => {
  if (currentView()?.kind === "trash") return;
  modify("trash", ["TRASH"], ["INBOX", "SPAM"]);
};

export const spam = () => {
  if (currentView()?.kind === "spam") return;
  modify("spam", ["SPAM"], ["INBOX", "TRASH"]);
};

export const snooze = (at: number) =>
  modify("snooze", [], ["INBOX"], { snooze: { kind: "set", at } });

const allHave = (field: "starred" | "unread") => {
  const t = targets();
  return t.length > 0 && t.every((id) => rowOf(String(id))?.[field]);
};

export const toggleStar = () => {
  if (allHave("starred")) modify("unstar", [], ["STARRED"]);
  else modify("star", ["STARRED"], []);
};

export const toggleRead = () => {
  if (allHave("unread")) modify("read", [], ["UNREAD"]);
  else modify("unread", ["UNREAD"], []);
};

/** `l`: if every target has the label it is removed from all, else added to all. */
export function applyLabel(labelId: string) {
  const t = targets();
  const all = t.length > 0 && t.every((id) => rowOf(String(id))?.labels.includes(labelId));
  if (all) modify("removeLabel", [], [labelId]);
  else modify("label", [labelId], []);
}

/** `y`: remove the current view's label when it can be removed. */
export function removeViewLabel() {
  const view = currentView();
  // Gmail does not let SENT be removed.
  const label = view && view.kind !== "sent" ? viewLabel(view) : null;
  if (!label) return;
  modify("removeLabel", [], [label]);
}

export function undo() {
  const c = useChrome.getState();
  if (c.account === null) return;
  const entry = undoTarget(useMirror.getState(), c.account);
  if (entry === null) return;
  // `z` on a send: the composer reopens as with the toast's Undo.
  const summary = accountSummary(useMirror.getState(), c.account);
  const outbox = summary?.undo.find((u) => u.id === entry)?.outbox;
  const row = outbox == null ? undefined : summary?.outbox.find((o) => o.id === outbox);
  if (row) {
    undoSend(row);
    return;
  }
  act({ type: "undo", account: c.account, entry });
}

/** Marks the open thread read; not an undo entry. */
export function markReadOnOpen(thread: number) {
  const row = rowOf(String(thread));
  if (row && !row.unread) return;
  modify("read", [], ["UNREAD"], { threads: [thread], undoable: false });
}

/** Closes search and restores the view and highlight it replaced (Q12). */
export function closeSearch() {
  const c = useChrome.getState();
  const before = c.beforeSearch ?? {
    where: { kind: "view", view: { kind: "inbox" } },
    highlight: null,
  };
  c.set({ search: null, beforeSearch: null, selection: [], inline: null });
  c.patchCurrent({ ...before, screen: { kind: "list" } });
}

export function goTo(where: Where) {
  const c = useChrome.getState();
  if (c.account === null) return;
  // Leaving the screen closes its composer as a second Esc would.
  appHooks.closeComposer();
  c.patchCurrent({ where, highlight: null, screen: { kind: "list" } });
  c.set({ selection: [], inline: null, search: where.kind === "search" ? c.search : null });
}

/**
 * Marking read on open is an action the UI sends after the conversation's
 * first load, or at once if it was prefetched.
 */
export function markReadOnOpenWhenLoaded(thread: number) {
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
    const screen = useChrome.getState().current()?.screen;
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
