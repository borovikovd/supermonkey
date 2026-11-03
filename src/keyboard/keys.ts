// The shortcut table (SPEC Keyboard). While compose, search, or the palette
// has focus, keystrokes go to that field and the table does not apply.

import {
  applyLabel,
  closeSearch,
  closeThread,
  goTo,
  markDone,
  markReadOnOpenWhenLoaded,
  openThread,
  removeViewLabel,
  returnToInbox,
  spam,
  toggleRead,
  toggleStar,
  trash,
  undo,
} from "../commands";
import { startCompose, startReply } from "../compose/start";
import type { View } from "../ipc/types";
import { useChrome } from "../store/chrome";
import { measure } from "./meter";
import {
  extendSelection,
  goSplit,
  jumpSplit,
  moveHighlight,
  selectMessage,
  toggleExpand,
  toggleExpandAll,
  toggleSelect,
} from "./nav";
import { appHooks, conversationRef } from "./refs";

export { applyLabel };

const isTyping = (el: EventTarget | null) => {
  if (!(el instanceof HTMLElement)) return false;
  return (
    el.isContentEditable ||
    el.tagName === "INPUT" ||
    el.tagName === "TEXTAREA" ||
    el.tagName === "SELECT"
  );
};

/** `g` then one of these keys opens that view. */
const G_VIEWS: Partial<Record<string, Exclude<View, { kind: "label" }>["kind"]>> = {
  e: "done",
  h: "snoozed",
  s: "starred",
  t: "sent",
  d: "drafts",
  "#": "trash",
  "!": "spam",
};

function handleG(key: string): boolean {
  useChrome.getState().set({ gPending: false });
  const view = G_VIEWS[key];
  if (view) {
    goTo({ kind: "view", view: { kind: view } });
    return true;
  }
  switch (key) {
    case "l":
      appHooks.openPalette({ kind: "goLabel" });
      return true;
    case "i":
      goSplit("important", appHooks.splitIdOf);
      return true;
    case "o":
      goSplit("other", appHooks.splitIdOf);
      return true;
  }
  return false;
}

/** Ends a `g` prefix that no second key followed. */
let gTimer: ReturnType<typeof setTimeout> | undefined;

/**
 * Esc closes the innermost thing: a focused link, then the conversation, then
 * the selection, then search results. onKeyDown closes an open sheet first;
 * the palette, the search field, and compose handle their own Esc.
 */
function escape() {
  const c = useChrome.getState();
  if (c.link) {
    c.set({ link: null });
    return;
  }
  if (c.current()?.screen.kind === "conversation") {
    closeThread();
    return;
  }
  if (c.selection.length > 0) {
    c.set({ selection: [] });
    return;
  }
  if (c.search) closeSearch();
}

export function onKeyDown(e: KeyboardEvent) {
  const start = performance.now();
  const c = useChrome.getState();
  if (c.account === null) return;
  const cur = c.current();

  // Account switching works everywhere, even while typing; it closes the
  // composer as a second Esc would.
  if (e.ctrlKey && !e.metaKey && /^Digit[1-9]$/.test(e.code)) {
    e.preventDefault();
    appHooks.switchAccount(Number(e.code.slice(5)) - 1);
    measure("account", start);
    return;
  }
  if (e.metaKey && e.key === ",") {
    e.preventDefault();
    c.set({ sheet: "settings", palette: null });
    return;
  }
  if (c.palette || isTyping(e.target) || c.inline || cur?.screen.kind === "compose") {
    // Owned by the palette, a field, or the composer.
    return;
  }
  if (c.sheet) {
    if (e.key === "Escape") {
      e.preventDefault();
      c.set({ sheet: null });
    }
    return;
  }
  if (e.metaKey && e.key.toLowerCase() === "k") {
    e.preventDefault();
    appHooks.openPalette({ kind: "commands" });
    measure("palette", start);
    return;
  }
  if (e.metaKey || e.altKey || (e.ctrlKey && e.key !== "Tab")) return;

  if (c.gPending) {
    e.preventDefault();
    if (handleG(e.key)) measure("view", start);
    return;
  }

  const inConv = cur?.screen.kind === "conversation";
  const data = conversationRef.data;
  if (inConv && appHooks.linkKey(e)) return;

  let handled = true;
  switch (e.key) {
    case "j":
      moveHighlight(1);
      measure(inConv ? "open" : "j", start);
      break;
    case "k":
      moveHighlight(-1);
      measure(inConv ? "open" : "k", start);
      break;
    case "J":
      extendSelection(1);
      measure("x", start);
      break;
    case "K":
      extendSelection(-1);
      measure("x", start);
      break;
    case "x":
      if (!inConv) toggleSelect();
      measure("x", start);
      break;
    case "Enter":
      if (inConv) {
        startReply("replyAll");
        measure("replyInline", start);
      } else if (cur?.highlight) {
        const view = cur.where.kind === "view" ? cur.where.view : null;
        if (view?.kind === "drafts") {
          startReply("resumeDraft", cur.highlight);
        } else {
          openThread(Number(cur.highlight));
          markReadOnOpenWhenLoaded(Number(cur.highlight));
          measure("open", start);
        }
      }
      break;
    case "Tab":
      if (!inConv) jumpSplit(e.shiftKey ? -1 : 1);
      break;
    case "e":
      markDone();
      measure("e", start);
      break;
    case "E":
      returnToInbox();
      measure("e", start);
      break;
    case "h":
      if (targetsExist()) appHooks.openPalette({ kind: "snooze" });
      break;
    case "c":
      startCompose();
      break;
    case "r":
      startReply("reply");
      measure(inConv ? "replyInline" : "replyFromList", start);
      break;
    case "f":
      startReply("forward");
      measure(inConv ? "replyInline" : "replyFromList", start);
      break;
    case "/":
      appHooks.focusSearch();
      measure("search", start);
      break;
    case "z":
      undo();
      measure("z", start);
      break;
    case "s":
      toggleStar();
      measure("s", start);
      break;
    case "u":
      toggleRead();
      measure("u", start);
      break;
    case "#":
      trash();
      measure("#", start);
      break;
    case "!":
      spam();
      measure("!", start);
      break;
    case "l":
      if (targetsExist()) appHooks.openPalette({ kind: "labels" });
      break;
    case "y":
      removeViewLabel();
      measure("e", start);
      break;
    case "n":
      if (inConv && data) selectMessage(data, 1);
      break;
    case "p":
      if (inConv && data) selectMessage(data, -1);
      break;
    case "o":
      if (inConv && data) toggleExpand(data);
      break;
    case "O":
      if (inConv) toggleExpandAll();
      break;
    case "g":
      c.set({ gPending: true });
      // A new `g` restarts the wait; an older timer must not end its prefix.
      clearTimeout(gTimer);
      gTimer = setTimeout(() => useChrome.getState().set({ gPending: false }), 1500);
      break;
    case "?":
      c.set({ sheet: "shortcuts" });
      break;
    case "Escape":
      escape();
      break;
    default:
      handled = false;
  }
  if (handled) e.preventDefault();
}

function targetsExist() {
  const c = useChrome.getState();
  const cur = c.current();
  return c.selection.length > 0 || cur?.screen.kind === "conversation" || cur?.highlight != null;
}
