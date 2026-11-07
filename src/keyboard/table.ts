// The command table (SPEC Keyboard): every command once, with its label, key
// binding, where it works (`when`), and what it does (`run`). The key
// dispatcher, the palette, the shortcut sheet, the hint bar, and the buttons
// that share a shortcut all read it.

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
import {
  blurComposer,
  closeComposerOnScreen,
  composeState,
  discardOnScreen,
  mountedComposer,
  sendOnScreen,
  useComposeState,
  type ComposeState,
} from "../compose/session";
import { notSent } from "../compose/sends";
import { startCompose, startReply } from "../compose/start";
import { addAccount, act, subIdFor } from "../ipc/bridge";
import type { View } from "../ipc/types";
import {
  closeConversation,
  closeSearch,
  closeSheet,
  goTo,
  goToSplit,
  openConversation,
  openPalette,
  openSearch,
  openSheet,
  showSearchResults,
  switchAccountAt,
} from "../navigation";
import { useChrome } from "../store/chrome";
import { conversationData, listKey } from "../store/current";
import { useMirror } from "../store/mirror";
import {
  accountSummary,
  appState,
  undoTarget,
  viewLabel,
  viewList,
  type MirrorState,
} from "../store/mirror-core";
import { reportSignIn, signInAgainOnScreen } from "../ui/signIn";
import { bindingChips, bindingText, chordMatches, parseBinding, type Binding } from "./keymap";
import { clearLink, cycleLink, openLink } from "./links";
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
import { measure } from "./meter";

// ------------------------------------------------------------ context

/** What decides where a command works, read from the stores. */
export type Ctx = {
  account: number | null;
  /** The account on screen needs Sign in again: its mail is not on screen. */
  signedOut: boolean;
  screen: "list" | "conversation";
  /** A composer on screen: a reply in the conversation, or the `c` screen. */
  composer: "inline" | "screen" | null;
  composeBlurred: boolean;
  /** The list's view; null for search results. */
  view: View | null;
  /** A sheet (Settings, Split Inbox settings, the shortcut sheet) is open. */
  sheet: boolean;
  searchOpen: boolean;
  /** The search field has focus. */
  searchField: boolean;
  /** Mail on screen to act on: the selection shown, the open conversation, or the highlighted row. */
  targets: boolean;
  /** The list on screen has rows. */
  rows: boolean;
  /** The highlighted row is shown. */
  highlight: boolean;
  /** Visible splits in a split Inbox. */
  splits: number;
  /** A link in the open conversation was reached with Tab. */
  link: boolean;
  selection: boolean;
  /** The open conversation has loaded. */
  loaded: boolean;
  undo: boolean;
  /** The account has user labels. */
  labels: boolean;
  accounts: number;
  muted: boolean;
};

type ChromeState = ReturnType<typeof useChrome.getState>;

let cached: { chrome: ChromeState; mirror: MirrorState; compose: ComposeState; ctx: Ctx } | null =
  null;

/** The context for these store states. */
function contextOf(chrome: ChromeState, mirror: MirrorState, compose: ComposeState): Ctx {
  if (cached?.chrome === chrome && cached.mirror === mirror && cached.compose === compose) {
    return cached.ctx;
  }
  const { account } = chrome;
  const cur = account === null ? undefined : chrome.perAccount[account];
  const app = appState(mirror);
  const summary = account === null ? null : accountSummary(mirror, account);
  const subId = account !== null && cur ? subIdFor(listKey(account, cur.where)) : undefined;
  const entries = subId ? viewList(mirror, subId).ids : [];
  const shown = new Set(entries.map((e) => e.key));
  const view = cur?.where.kind === "view" ? cur.where.view : null;
  const screen = cur?.screen ?? { kind: "list" as const };
  const highlight = cur?.highlight != null && shown.has(cur.highlight);
  const selected = chrome.selection.filter((k) => shown.has(k) && !Number.isNaN(Number(k)));
  const targets =
    screen.kind === "conversation" ||
    selected.length > 0 ||
    (highlight && view?.kind !== "drafts" && !Number.isNaN(Number(cur?.highlight)));
  const splits = new Set(entries.map((e) => e.split).filter((s) => s !== null)).size;
  const onScreen = compose.onScreen;
  const ctx: Ctx = {
    account,
    signedOut: app?.accounts.find((a) => a.id === account)?.needsSignin ?? false,
    screen: screen.kind,
    composer: onScreen === null ? null : onScreen.thread === null ? "screen" : "inline",
    composeBlurred: compose.blurred,
    view,
    sheet: chrome.sheet !== null,
    searchOpen: chrome.search !== null,
    searchField: chrome.search?.focus === "field",
    targets,
    rows: entries.length > 0,
    highlight,
    splits: view?.kind === "inbox" ? splits : 0,
    link: chrome.link !== null,
    selection: chrome.selection.length > 0,
    loaded:
      screen.kind === "conversation" &&
      conversationData(mirror, screen.thread, screen.trashMode) !== null,
    undo: account !== null && undoTarget(mirror, account) !== null,
    labels: summary?.labels.some((l) => !l.system) ?? false,
    accounts: app?.accounts.length ?? 0,
    muted: summary?.account.muted ?? false,
  };
  cached = { chrome, mirror, compose, ctx };
  return ctx;
}

/** The context now, outside React. */
export function context(): Ctx {
  return contextOf(useChrome.getState(), useMirror.getState(), composeState());
}

/** The context, kept current in a component. */
export function useCtx(): Ctx {
  const chrome = useChrome((s) => s);
  const mirror = useMirror((s) => s);
  const compose = useComposeState((s) => s);
  return contextOf(chrome, mirror, compose);
}

// ------------------------------------------------------------ where commands work

/** Mail is on screen: an account signed in, no composer over it. */
const mailScreen = (c: Ctx) => c.account !== null && !c.signedOut && c.composer === null;
const onList = (c: Ctx) => mailScreen(c) && c.screen === "list";
const inConversation = (c: Ctx) => mailScreen(c) && c.screen === "conversation";
/** A mail action has something to act on (SPEC Keyboard). */
const acts = (c: Ctx) => mailScreen(c) && c.targets;
/** Views and settings of the account on screen, also from the rail while composing. */
const accountScreen = (c: Ctx) => c.account !== null && !c.signedOut;
const anyScreen = (c: Ctx) => c.account !== null;
const composing = (c: Ctx) => c.composer !== null;

const isView = (c: Ctx, kind: View["kind"]) => c.view?.kind === kind;

// ------------------------------------------------------------ the table

/**
 * Where a key is looked up (src/keyboard/dispatch.ts): keys that work
 * everywhere, an open sheet, the composer, the search field, and the
 * shortcut table, which applies only on mail.
 */
export type Scope = "global" | "sheet" | "compose" | "search" | "mail";

/** The shortcut sheet's sections, in order. */
export const GROUPS = ["Navigate", "Act", "Write", "Go to"] as const;

type Def = {
  label: string;
  /** The label when it depends on state (Mute / Unmute). */
  dynamicLabel?: (c: Ctx) => string;
  /** A button's name, when shorter than the label. */
  button?: string;
  /** Key bindings; the first is the one shown. */
  keys?: string | readonly string[];
  scope?: Scope;
  /** The shortcut sheet's section; every command with keys has one. */
  group?: (typeof GROUPS)[number];
  /** Listed in the palette; a function runs there instead of `run` (a picker for a key that acts at once). */
  palette?: true | (() => void);
  /** The hint bar's words after the key, e.g. `to Mark Done`. */
  hint?: string;
  when: (c: Ctx) => boolean;
  /** Runs the command; a key passes its event. */
  run: (c: Ctx, e?: KeyboardEvent) => void;
  /** The performance budget the key is measured against (SPEC Performance). */
  meter?: string | ((c: Ctx) => string);
};

const view = (kind: Exclude<View["kind"], "label">) => () => goTo({ kind });

/** Esc on mail closes the innermost thing: a link, the conversation, the selection, then search. */
function escape(c: Ctx) {
  if (c.link) clearLink();
  else if (c.screen === "conversation") closeConversation();
  else if (c.selection) useChrome.getState().setSelection([]);
  else if (c.searchOpen) closeSearch();
}

const DEFS = {
  // An open sheet.
  "close-sheet": {
    label: "Close the sheet",
    keys: "Escape",
    scope: "sheet",
    when: (c) => c.sheet,
    run: closeSheet,
  },

  // The sign-in screen.
  signin: {
    label: "Sign in again",
    keys: "Enter",
    group: "Navigate",
    palette: true,
    hint: "to sign in",
    when: (c) => c.signedOut && c.composer === null,
    run: signInAgainOnScreen,
  },

  // Mail actions, in the palette's order.
  done: {
    label: "Mark done",
    button: "Mark Done",
    keys: "e",
    group: "Act",
    palette: true,
    hint: "to Mark Done",
    when: (c) => acts(c) && !isView(c, "done"),
    run: markDone,
    meter: "e",
  },
  inbox: {
    label: "Return to inbox",
    keys: "Shift+E",
    group: "Act",
    palette: true,
    when: (c) => acts(c) && !isView(c, "inbox"),
    run: returnToInbox,
    meter: "e",
  },
  snooze: {
    label: "Snooze",
    keys: "h",
    group: "Act",
    palette: true,
    hint: "to snooze",
    when: acts,
    run: () => openPalette({ kind: "snooze" }),
  },
  trash: {
    label: "Trash",
    keys: "#",
    group: "Act",
    palette: true,
    when: (c) => acts(c) && !isView(c, "trash"),
    run: trash,
    meter: "#",
  },
  spam: {
    label: "Mark as spam",
    keys: "!",
    group: "Act",
    // The palette offers spam with a block; `!` marks spam at once.
    palette: () => openPalette({ kind: "spam" }),
    when: (c) => acts(c) && !isView(c, "spam"),
    run: spam,
    meter: "!",
  },
  block: {
    label: "Block",
    palette: true,
    when: acts,
    run: () => openPalette({ kind: "block" }),
  },
  blocked: {
    label: "Blocked senders",
    palette: true,
    when: mailScreen,
    run: () => openPalette({ kind: "blocked" }),
  },
  star: {
    label: "Star or unstar",
    keys: "s",
    group: "Act",
    palette: true,
    when: acts,
    run: toggleStar,
    meter: "s",
  },
  unsubscribe: {
    label: "Unsubscribe",
    keys: "Meta+U",
    group: "Act",
    palette: true,
    when: acts,
    run: () => openPalette({ kind: "unsubscribe" }),
  },
  read: {
    label: "Mark read or unread",
    keys: "u",
    group: "Act",
    palette: true,
    when: acts,
    run: toggleRead,
    meter: "u",
  },
  "select-all": {
    label: "Select all",
    keys: "Meta+Shift+A",
    group: "Navigate",
    palette: true,
    when: (c) => onList(c) && c.rows,
    run: () => selectAll(false),
    meter: "x",
  },
  "select-from-here": {
    label: "Select all from here",
    keys: "Meta+A",
    group: "Navigate",
    palette: true,
    when: (c) => onList(c) && c.highlight,
    run: () => selectAll(true),
    meter: "x",
  },
  label: {
    label: "Add or remove label",
    keys: "l",
    group: "Act",
    palette: true,
    when: acts,
    run: () => openPalette({ kind: "labels" }),
  },
  unlabel: {
    label: "Remove current label",
    keys: "y",
    group: "Act",
    palette: true,
    // Gmail does not let SENT be removed.
    when: (c) => acts(c) && c.view !== null && c.view.kind !== "sent" && viewLabel(c.view) !== null,
    run: removeViewLabel,
    meter: "e",
  },
  undo: {
    label: "Undo",
    keys: "z",
    group: "Act",
    palette: true,
    when: (c) => mailScreen(c) && c.undo,
    run: undo,
    meter: "z",
  },

  // Writing.
  reply: {
    label: "Reply",
    keys: "r",
    group: "Write",
    palette: true,
    hint: "to Reply",
    when: (c) => inConversation(c) || (onList(c) && c.highlight && !isView(c, "drafts")),
    run: () => startReply("reply"),
    meter: (c) => (c.screen === "conversation" ? "replyInline" : "replyFromList"),
  },
  replyall: {
    label: "Reply all",
    keys: "Enter",
    group: "Write",
    palette: true,
    hint: "to Reply all",
    when: (c) => inConversation(c) && !c.link,
    run: () => startReply("replyAll"),
    meter: "replyInline",
  },
  forward: {
    label: "Forward",
    keys: "f",
    group: "Write",
    palette: true,
    when: (c) => inConversation(c) || (onList(c) && c.highlight && !isView(c, "drafts")),
    run: () => startReply("forward"),
    meter: (c) => (c.screen === "conversation" ? "replyInline" : "replyFromList"),
  },
  compose: {
    label: "Compose",
    keys: "c",
    group: "Write",
    palette: true,
    hint: "to compose",
    when: mailScreen,
    run: startCompose,
  },
  search: {
    label: "Search",
    keys: "/",
    group: "Navigate",
    palette: true,
    when: mailScreen,
    run: openSearch,
    meter: "search",
  },

  // Views.
  "g-inbox": {
    label: "Go to Inbox",
    button: "Inbox",
    palette: true,
    when: accountScreen,
    run: view("inbox"),
    meter: "view",
  },
  "g-imp": {
    label: "Go to Important",
    keys: "g i",
    group: "Go to",
    palette: true,
    when: accountScreen,
    run: () => goToSplit("important"),
    meter: "view",
  },
  "g-other": {
    label: "Go to Other",
    keys: "g o",
    group: "Go to",
    palette: true,
    when: accountScreen,
    run: () => goToSplit("other"),
    meter: "view",
  },
  "g-done": {
    label: "Go to Done",
    button: "Done",
    keys: "g e",
    group: "Go to",
    palette: true,
    when: accountScreen,
    run: view("done"),
    meter: "view",
  },
  "g-all": {
    label: "Go to All Mail",
    keys: "g a",
    group: "Go to",
    palette: true,
    when: accountScreen,
    run: view("allMail"),
    meter: "view",
  },
  "g-snoozed": {
    label: "Go to Snoozed",
    button: "Snoozed",
    keys: "g h",
    group: "Go to",
    palette: true,
    when: accountScreen,
    run: view("snoozed"),
    meter: "view",
  },
  "g-starred": {
    label: "Go to Starred",
    button: "Starred",
    keys: "g s",
    group: "Go to",
    palette: true,
    when: accountScreen,
    run: view("starred"),
    meter: "view",
  },
  "g-sent": {
    label: "Go to Sent",
    button: "Sent",
    keys: "g t",
    group: "Go to",
    palette: true,
    when: accountScreen,
    run: view("sent"),
    meter: "view",
  },
  "g-drafts": {
    label: "Go to Drafts",
    button: "Drafts",
    keys: "g d",
    group: "Go to",
    palette: true,
    when: accountScreen,
    run: view("drafts"),
    meter: "view",
  },
  "g-trash": {
    label: "Go to Trash",
    keys: "g #",
    group: "Go to",
    palette: true,
    when: accountScreen,
    run: view("trash"),
    meter: "view",
  },
  "g-spam": {
    label: "Go to Spam",
    keys: "g !",
    group: "Go to",
    palette: true,
    when: accountScreen,
    run: view("spam"),
    meter: "view",
  },
  "g-label": {
    label: "Go to label",
    keys: "g l",
    group: "Go to",
    palette: true,
    when: (c) => accountScreen(c) && c.labels,
    run: () => openPalette({ kind: "goLabel" }),
  },

  // Split Inbox, accounts, and the app.
  "split-add": {
    label: "Add to split",
    palette: true,
    when: acts,
    run: () => openPalette({ kind: "addToSplit" }),
  },
  "split-imp": {
    label: "Move to Important",
    palette: true,
    when: acts,
    run: () => openPalette({ kind: "addToSplit", preset: "important" }),
  },
  "split-other": {
    label: "Move to Other",
    palette: true,
    when: acts,
    run: () => openPalette({ kind: "addToSplit", preset: "other" }),
  },
  "split-settings": {
    label: "Split Inbox settings",
    palette: true,
    when: mailScreen,
    run: () => openSheet("splits"),
  },
  switch: {
    label: "Switch account",
    palette: true,
    when: (c) => c.accounts > 1,
    run: () => openPalette({ kind: "switchAccount" }),
  },
  "account-n": {
    label: "Switch account",
    keys: "Control+1…9",
    scope: "global",
    group: "Navigate",
    when: anyScreen,
    run: (_c, e) => {
      if (e) switchAccountAt(Number(e.code.slice(5)) - 1);
    },
    meter: "account",
  },
  "add-account": {
    label: "Add account",
    palette: true,
    when: anyScreen,
    run: () => reportSignIn(addAccount()),
  },
  mute: {
    label: "Mute notifications",
    dynamicLabel: (c) => (c.muted ? "Unmute notifications" : "Mute notifications"),
    palette: true,
    when: anyScreen,
    run: (c) => {
      if (c.account !== null)
        act({ type: "setSetting", setting: "muted", account: c.account, on: !c.muted });
    },
  },
  settings: {
    label: "Settings",
    keys: "Meta+,",
    scope: "global",
    group: "Navigate",
    palette: true,
    when: anyScreen,
    run: () => openSheet("settings"),
  },
  shortcuts: {
    label: "Keyboard shortcuts",
    keys: "?",
    group: "Navigate",
    palette: true,
    when: mailScreen,
    run: () => openSheet("shortcuts"),
  },
  palette: {
    label: "Commands",
    keys: "Meta+K",
    group: "Navigate",
    hint: "for commands",
    when: (c) => anyScreen(c) && c.composer === null,
    run: () => openPalette({ kind: "commands" }),
    meter: "palette",
  },

  // Moving around the list and the conversation; not in the palette.
  next: {
    label: "Next conversation",
    keys: "j",
    group: "Navigate",
    when: (c) => mailScreen(c) && c.rows,
    run: () => moveHighlight(1),
    meter: (c) => (c.screen === "conversation" ? "open" : "j"),
  },
  prev: {
    label: "Previous conversation",
    keys: "k",
    group: "Navigate",
    when: (c) => mailScreen(c) && c.rows,
    run: () => moveHighlight(-1),
    meter: (c) => (c.screen === "conversation" ? "open" : "k"),
  },
  open: {
    label: "Open conversation",
    keys: "Enter",
    group: "Navigate",
    when: (c) => onList(c) && c.highlight && !isView(c, "drafts"),
    run: () => {
      const highlight = useChrome.getState().current()?.highlight;
      if (highlight != null) openConversation(Number(highlight));
    },
    meter: "open",
  },
  "open-draft": {
    label: "Open draft",
    keys: "Enter",
    group: "Navigate",
    hint: "to open draft",
    when: (c) => onList(c) && c.highlight && isView(c, "drafts"),
    run: () => {
      const highlight = useChrome.getState().current()?.highlight;
      if (highlight != null) startReply("resumeDraft", highlight);
    },
  },
  escape: {
    label: "Back to the list, or close",
    keys: "Escape",
    group: "Navigate",
    when: (c) =>
      mailScreen(c) && (c.link || c.screen === "conversation" || c.selection || c.searchOpen),
    run: escape,
  },
  back: {
    label: "Back",
    when: (c) => c.composer === "screen" || (c.account !== null && c.screen === "conversation"),
    run: (c) => (c.composer === "screen" ? closeComposerOnScreen() : closeConversation()),
  },
  "next-split": {
    label: "Next split",
    keys: "Tab",
    group: "Navigate",
    when: (c) => onList(c) && c.splits > 1,
    run: () => jumpSplit(1),
  },
  "prev-split": {
    label: "Previous split",
    keys: "Shift+Tab",
    group: "Navigate",
    when: (c) => onList(c) && c.splits > 1,
    run: () => jumpSplit(-1),
  },
  "next-link": {
    label: "Next link in the conversation",
    keys: "Tab",
    group: "Navigate",
    when: inConversation,
    run: () => cycleLink(false),
  },
  "prev-link": {
    label: "Previous link",
    keys: "Shift+Tab",
    group: "Navigate",
    when: inConversation,
    run: () => cycleLink(true),
  },
  "open-link": {
    label: "Open the link",
    keys: "Enter",
    group: "Navigate",
    when: (c) => inConversation(c) && c.link,
    run: openLink,
  },
  "next-message": {
    label: "Next message",
    keys: "n",
    group: "Navigate",
    when: (c) => inConversation(c) && c.loaded,
    run: (c) => withConversation(c, (d) => selectMessage(d, 1)),
  },
  "prev-message": {
    label: "Previous message",
    keys: "p",
    group: "Navigate",
    when: (c) => inConversation(c) && c.loaded,
    run: (c) => withConversation(c, (d) => selectMessage(d, -1)),
  },
  "toggle-message": {
    label: "Expand or collapse message",
    keys: "o",
    group: "Navigate",
    when: (c) => inConversation(c) && c.loaded,
    run: (c) => withConversation(c, toggleExpand),
  },
  "expand-all": {
    label: "Expand everything",
    keys: "Shift+O",
    group: "Navigate",
    when: inConversation,
    run: toggleExpandAll,
  },
  select: {
    label: "Select",
    keys: "x",
    group: "Navigate",
    when: (c) => onList(c) && c.highlight,
    run: toggleSelect,
    meter: "x",
  },
  "extend-down": {
    label: "Extend the selection down",
    keys: "Shift+J",
    group: "Navigate",
    when: (c) => onList(c) && c.rows,
    run: () => extendSelection(1),
    meter: "x",
  },
  "extend-up": {
    label: "Extend the selection up",
    keys: "Shift+K",
    group: "Navigate",
    when: (c) => onList(c) && c.rows,
    run: () => extendSelection(-1),
    meter: "x",
  },

  // The search field.
  "search-results": {
    label: "See results",
    keys: ["Enter", "ArrowDown"],
    scope: "search",
    group: "Navigate",
    hint: "to see results",
    when: (c) => c.searchField,
    run: showSearchResults,
  },
  "search-close": {
    label: "Close search",
    keys: "Escape",
    scope: "search",
    group: "Navigate",
    hint: "to close search",
    when: (c) => c.searchOpen,
    run: closeSearch,
  },

  // The composer (SPEC Compose); the palette's draft actions first.
  send: {
    label: "Send",
    keys: "Meta+Enter",
    scope: "compose",
    group: "Write",
    palette: true,
    hint: "to Send",
    when: composing,
    run: () => sendOnScreen(notSent),
  },
  attach: {
    label: "Attach a file",
    button: "Attach",
    keys: "Meta+Shift+A",
    scope: "compose",
    group: "Write",
    palette: true,
    when: composing,
    run: () => mountedComposer()?.attach(),
  },
  discard: {
    label: "Discard draft",
    button: "Discard",
    keys: "Meta+Shift+,",
    scope: "compose",
    group: "Write",
    palette: true,
    when: composing,
    run: discardOnScreen,
  },
  "focus-to": {
    label: "To",
    keys: "Meta+Shift+O",
    scope: "compose",
    group: "Write",
    when: composing,
    run: () => mountedComposer()?.focus("to"),
  },
  "focus-cc": {
    label: "Cc",
    keys: "Meta+Shift+C",
    scope: "compose",
    group: "Write",
    when: composing,
    run: () => mountedComposer()?.focus("cc"),
  },
  "focus-bcc": {
    label: "Bcc",
    keys: "Meta+Shift+B",
    scope: "compose",
    group: "Write",
    when: composing,
    run: () => mountedComposer()?.focus("bcc"),
  },
  from: {
    label: "From",
    keys: "Meta+Shift+F",
    scope: "compose",
    group: "Write",
    when: composing,
    run: () => openPalette({ kind: "from" }),
  },
  "focus-subject": {
    label: "Subject",
    keys: "Meta+Shift+S",
    scope: "compose",
    group: "Write",
    when: composing,
    run: () => mountedComposer()?.focus("subject"),
  },
  "focus-body": {
    label: "Message",
    keys: "Meta+Shift+M",
    scope: "compose",
    group: "Write",
    when: composing,
    run: () => mountedComposer()?.focus("body"),
  },
  "compose-k": {
    label: "Insert a link, or draft actions",
    keys: "Meta+K",
    scope: "compose",
    group: "Write",
    hint: "for commands",
    when: composing,
    run: (c) => {
      const handle = mountedComposer();
      if (!c.composeBlurred && handle?.editorFocused()) handle.openLink();
      else openPalette({ kind: "commands" });
    },
  },
  "compose-escape": {
    label: "Leave the editor, then close",
    keys: "Escape",
    scope: "compose",
    group: "Write",
    hint: "to close",
    when: composing,
    run: (c) => {
      if (mountedComposer()?.closeLink()) return;
      // The first Esc blurs the editor and leaves compose on screen.
      if (!c.composeBlurred) blurComposer();
      else closeComposerOnScreen();
    },
  },
  "compose-edit": {
    label: "Back to the editor",
    keys: "Enter",
    scope: "compose",
    group: "Write",
    hint: "to edit",
    when: (c) => composing(c) && c.composeBlurred,
    run: () => mountedComposer()?.focus("body"),
  },
} satisfies Record<string, Def>;

export type CommandId = keyof typeof DEFS;

export type Command = Def & { id: string; scope: Scope; bindings: Binding[] };

function withConversation(
  c: Ctx,
  run: (data: NonNullable<ReturnType<typeof conversationData>>) => void,
) {
  const screen = useChrome.getState().current()?.screen;
  if (c.screen !== "conversation" || screen?.kind !== "conversation") return;
  const data = conversationData(useMirror.getState(), screen.thread, screen.trashMode);
  if (data) run(data);
}

function toCommand(id: string, def: Def): Command {
  const keys = def.keys === undefined ? [] : typeof def.keys === "string" ? [def.keys] : def.keys;
  return { ...def, id, scope: def.scope ?? "mail", bindings: keys.map(parseBinding) };
}

/** Every command, in table order. */
export const COMMANDS: readonly Command[] = Object.entries(DEFS).map(([id, def]: [string, Def]) =>
  toCommand(id, def),
);

const BY_ID = new Map(COMMANDS.map((c) => [c.id, c]));

export function command(id: CommandId): Command {
  const c = BY_ID.get(id);
  if (!c) throw new Error(`no command ${id}`);
  return c;
}

export const labelOf = (cmd: Command, c: Ctx) => cmd.dynamicLabel?.(c) ?? cmd.label;

/** The shortcut as text, `⌘K`; empty without one. */
export const keyText = (cmd: Command) => (cmd.bindings[0] ? bindingText(cmd.bindings[0]) : "");

/** The palette's chips for the shortcut. */
export const keyChips = (cmd: Command) => (cmd.bindings[0] ? bindingChips(cmd.bindings[0]) : []);

/** The `⌃ n` chips of the n-th account in the switcher. */
export const accountChips = (n: number) => bindingChips(parseBinding(`Control+${n}`));

/** A button's title: its name and shortcut, e.g. `Mark Done (E)`. */
export function titleOf(cmd: Command): string {
  const name = cmd.button ?? cmd.label;
  const keys = keyText(cmd);
  return keys ? `${name} (${keys})` : name;
}

/** A hint as the hint bar shows it, e.g. `E to Mark Done`. */
export const hintOf = (cmd: Command) => `${keyText(cmd)} ${cmd.hint ?? cmd.label}`;

/** A hint in key symbols, e.g. `⌘↵ to Send`, as the composer's footer shows it. */
export const shortHintOf = (cmd: Command) => `${keyChips(cmd).join("")} ${cmd.hint ?? cmd.label}`;

// ------------------------------------------------------------ readers

/** The palette's commands where it opened: the draft actions while a composer is on screen. */
export function paletteCommands(c: Ctx): Command[] {
  return COMMANDS.filter(
    (cmd) =>
      cmd.palette !== undefined &&
      cmd.when(c) &&
      (c.composer === null) === (cmd.scope !== "compose"),
  );
}

/** Choosing a command in the palette. */
export function runFromPalette(cmd: Command) {
  if (typeof cmd.palette === "function") cmd.palette();
  else cmd.run(context());
}

/** The hint bar on this screen (SPEC Layout), leaving out hints whose key would do nothing. */
export function hints(c: Ctx): Command[] {
  const ids: CommandId[] =
    c.composer !== null
      ? ["send", "compose-escape", "compose-k"]
      : c.signedOut
        ? ["signin", "palette"]
        : c.searchField
          ? ["search-results", "search-close"]
          : c.screen === "conversation"
            ? ["done", "reply", "replyall", "palette"]
            : isView(c, "drafts")
              ? ["open-draft", "compose", "palette"]
              : ["done", "snooze", "palette"];
  return ids.map(command).filter((cmd) => cmd.when(c));
}

/** The shortcut sheet: every command with a shortcut, by section. */
export function sheet(): { group: (typeof GROUPS)[number]; commands: Command[] }[] {
  return GROUPS.map((group) => ({
    group,
    commands: COMMANDS.filter((cmd) => cmd.group === group && cmd.bindings.length > 0),
  }));
}

/** A command for a button: its title, whether it works now, and how to run it. */
export function useCommand(id: CommandId) {
  const c = useCtx();
  const cmd = command(id);
  return { title: titleOf(cmd), enabled: cmd.when(c), run: () => cmd.run(context()) };
}

// ------------------------------------------------------------ keys

type Chord = Binding[number];

/** `g` was pressed: the first chord of a sequence, waiting for its second. */
let prefix: Chord | null = null;
let prefixTimer: ReturnType<typeof setTimeout> | undefined;

const sameChord = (a: Chord, b: Chord) =>
  a.key === b.key &&
  a.meta === b.meta &&
  a.shift === b.shift &&
  a.ctrl === b.ctrl &&
  a.alt === b.alt;

function runCommand(cmd: Command, c: Ctx, e: KeyboardEvent, start: number) {
  cmd.run(c, e);
  const meter = typeof cmd.meter === "function" ? cmd.meter(c) : cmd.meter;
  if (meter) measure(meter, start);
}

/**
 * Looks a key up in one scope's commands and runs the first that works now.
 * True when the key was taken: run, or on mail a plain key of the shortcut
 * table that does nothing here, which no browser default should get.
 */
export function runKey(scope: Scope, e: KeyboardEvent, c: Ctx, start: number): boolean {
  const inScope = COMMANDS.filter((cmd) => cmd.scope === scope);
  if (prefix !== null && scope === "mail") {
    const first = prefix;
    prefix = null;
    clearTimeout(prefixTimer);
    e.preventDefault();
    const cmd = inScope.find(
      (x) =>
        x.when(c) &&
        x.bindings.some(
          (b) => b[0] !== undefined && sameChord(b[0], first) && b[1] && chordMatches(b[1], e),
        ),
    );
    if (cmd) runCommand(cmd, c, e, start);
    return true;
  }
  // The first key of a sequence some command here would take.
  const starts = inScope.flatMap((x) =>
    x.when(c) ? x.bindings.filter((b) => b.length === 2 && b[0] && chordMatches(b[0], e)) : [],
  );
  const opener = starts[0]?.[0];
  if (opener) {
    e.preventDefault();
    prefix = opener;
    // A new prefix restarts the wait; an older timer must not end it.
    clearTimeout(prefixTimer);
    prefixTimer = setTimeout(() => {
      prefix = null;
    }, 1500);
    return true;
  }
  const matching = inScope.filter((x) =>
    x.bindings.some((b) => b.length === 1 && b[0] && chordMatches(b[0], e)),
  );
  const cmd = matching.find((x) => x.when(c));
  if (cmd) {
    e.preventDefault();
    runCommand(cmd, c, e, start);
    return true;
  }
  // A plain key: chords with Cmd stay with the browser, so `Cmd+A` in a
  // conversation still selects its text.
  if (scope === "mail" && matching.length > 0 && !e.metaKey && !e.ctrlKey && !e.altKey) {
    e.preventDefault();
    return true;
  }
  return false;
}
