// The shortcut table (SPEC Keyboard). While compose, search, or the palette
// has focus, keystrokes go to that field and the table does not apply.

import {
  markDone,
  removeViewLabel,
  returnToInbox,
  spam,
  toggleRead,
  toggleStar,
  trash,
  undo,
} from "../commands";
import { composerOnScreen } from "../compose/session";
import { startCompose, startReply, typeAhead } from "../compose/start";
import type { View } from "../ipc/types";
import {
  closeConversation,
  closeSearch,
  goTo,
  goToSplit,
  openConversation,
  openPalette,
  openSearch,
  openSheet,
  switchAccountAt,
} from "../navigation";
import { useChrome } from "../store/chrome";
import { openConversationData } from "../store/current";
import { needsSignin } from "../store/hooks";
import { signInAgainOnScreen } from "../ui/signIn";
import { clearLink, cycleLink, openLink } from "./links";
import { measure } from "./meter";
import {
  extendSelection,
  jumpSplit,
  moveHighlight,
  selectAll,
  selectMessage,
  toggleExpand,
  toggleExpandAll,
  toggleSelect,
} from "./nav";

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
  a: "allMail",
  h: "snoozed",
  s: "starred",
  t: "sent",
  d: "drafts",
  "#": "trash",
  "!": "spam",
};

/** `g` was pressed; waiting for the second key. */
let gPending = false;

function handleG(key: string): boolean {
  gPending = false;
  const view = G_VIEWS[key];
  if (view) {
    goTo({ kind: view });
    return true;
  }
  switch (key) {
    case "l":
      openPalette({ kind: "goLabel" });
      return true;
    case "i":
      goToSplit("important");
      return true;
    case "o":
      goToSplit("other");
      return true;
  }
  return false;
}

/** Ends a `g` prefix that no second key followed. */
let gTimer: ReturnType<typeof setTimeout> | undefined;

/** Tab cycles links in the open conversation; Enter opens one reached; Esc clears it. */
function linkKey(e: KeyboardEvent): boolean {
  const { link } = useChrome.getState();
  if (e.key === "Tab") cycleLink(e.shiftKey);
  else if (link && e.key === "Enter") openLink();
  else if (link && e.key === "Escape") clearLink();
  else return false;
  e.preventDefault();
  return true;
}

/**
 * Esc closes the innermost thing: a focused link, then the conversation, then
 * the selection, then search results. onKeyDown closes an open sheet first;
 * the palette, the search field, and compose handle their own Esc.
 */
function escape() {
  const c = useChrome.getState();
  if (c.link) {
    clearLink();
    return;
  }
  if (c.current()?.screen.kind === "conversation") {
    closeConversation();
    return;
  }
  if (c.selection.length > 0) {
    c.setSelection([]);
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
    switchAccountAt(Number(e.code.slice(5)) - 1);
    measure("account", start);
    return;
  }
  if (e.metaKey && e.key === ",") {
    e.preventDefault();
    openSheet("settings");
    return;
  }
  // Typed after r, f, or Enter, before the reply editor has focus: the reply's text.
  if (typeAhead(e)) return;
  if (c.palette || isTyping(e.target) || composerOnScreen()) {
    // Owned by the palette, a field, or the composer.
    return;
  }
  if (c.sheet) {
    if (e.key === "Escape") {
      e.preventDefault();
      c.closeSheet();
    }
    return;
  }
  if (
    e.metaKey &&
    !e.shiftKey &&
    e.key.toLowerCase() === "u" &&
    (cur?.screen.kind === "list" || cur?.screen.kind === "conversation") &&
    !needsSignin(c.account)
  ) {
    e.preventDefault();
    openPalette({ kind: "unsubscribe" });
    return;
  }
  if (e.metaKey && e.key.toLowerCase() === "k") {
    e.preventDefault();
    openPalette({ kind: "commands" });
    measure("palette", start);
    return;
  }
  if (
    e.metaKey &&
    !e.altKey &&
    e.key.toLowerCase() === "a" &&
    cur?.screen.kind === "list" &&
    !needsSignin(c.account)
  ) {
    e.preventDefault();
    selectAll(!e.shiftKey);
    measure("x", start);
    return;
  }
  if (e.metaKey || e.altKey || (e.ctrlKey && e.key !== "Tab")) return;
  // A signed-out account's screen only signs in; its mail is not on screen.
  if (needsSignin(c.account)) {
    if (e.key === "Enter") {
      e.preventDefault();
      signInAgainOnScreen();
    }
    return;
  }

  if (gPending) {
    e.preventDefault();
    if (handleG(e.key)) measure("view", start);
    return;
  }

  const inConv = cur?.screen.kind === "conversation";
  const data = openConversationData();
  if (inConv && linkKey(e)) return;

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
          openConversation(Number(cur.highlight));
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
      if (targetsExist()) openPalette({ kind: "snooze" });
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
      openSearch();
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
      if (targetsExist()) openPalette({ kind: "labels" });
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
      gPending = true;
      // A new `g` restarts the wait; an older timer must not end its prefix.
      clearTimeout(gTimer);
      gTimer = setTimeout(() => {
        gPending = false;
      }, 1500);
      break;
    case "?":
      openSheet("shortcuts");
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
