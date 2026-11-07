// Chrome state: account on screen, each account's last view and highlight,
// selected ids, palette, search, and the open conversation's view state (SPEC
// UI state). The composer on screen is compose state (src/compose/session.ts).
//
// Only named actions change it. They are plain state changes; the rules that
// go with a screen change (close the composer first, mark read on open) are in
// src/navigation.ts, which is what the rest of the UI calls.

import { create } from "zustand";

import type { View } from "../ipc/types";

export type Where = { kind: "view"; view: View } | { kind: "search"; query: string };

export type Screen =
  | { kind: "list" }
  | { kind: "conversation"; thread: number; trashMode: boolean };

export type PaletteMode =
  | { kind: "commands" }
  | { kind: "labels" }
  | { kind: "goLabel" }
  | { kind: "snooze" }
  | { kind: "unsubscribe" }
  | { kind: "block" }
  | { kind: "spam" }
  | { kind: "blocked" }
  | { kind: "switchAccount" }
  | { kind: "addToSplit"; preset?: "important" | "other" }
  | { kind: "from" };

export type Sheet = "shortcuts" | "settings" | "splits" | null;

export type AccountChrome = {
  where: Where;
  highlight: string | null;
  screen: Screen;
};

type ChromeState = {
  account: number | null;
  perAccount: Record<number, AccountChrome>;
  selection: string[];
  palette: PaletteMode | null;
  sheet: Sheet;
  search: { query: string; focus: "field" | "results" } | null;
  /** The view and highlight search replaced, restored when it closes. */
  beforeSearch: { where: Where; highlight: string | null } | null;
  /** Selected message in the open conversation. */
  selectedMessage: number | null;
  /** Expanded messages in the open conversation (set when it opens). */
  expanded: number[];
  /** Folded runs opened by the user. */
  unfolded: number[];
  expandAll: boolean;
  /** A link reached with Tab inside a message, as [messageId, linkIndex]. */
  link: [number, number] | null;
  /** A split shown by `g i` / `g o` even though it is empty and hidden. */
  forcedSplit: number | null;
};

type ChromeActions = {
  current: () => AccountChrome | null;
  /** The account on screen; switching closes the palette and search and clears the selection. */
  setAccount: (account: number | null) => void;
  /** A view or a search on the list: no highlight yet, no selection. */
  showList: (where: Where) => void;
  /** The list of the current view, leaving an open conversation. */
  leaveConversation: () => void;
  /** Opens a conversation from the list row `row`, which takes the highlight. */
  showConversation: (thread: number, trashMode: boolean, row: string) => void;
  setHighlight: (key: string | null) => void;
  setSelection: (keys: string[]) => void;
  forceSplit: (split: number | null) => void;

  openSearch: () => void;
  setSearchQuery: (query: string) => void;
  setSearchFocus: (focus: "field" | "results") => void;
  /** Closes search and restores the view and highlight it replaced. */
  closeSearch: () => void;

  openPalette: (mode: PaletteMode) => void;
  closePalette: () => void;
  openSheet: (sheet: Exclude<Sheet, null>) => void;
  closeSheet: () => void;

  /** The open conversation's first load: what is expanded and selected. */
  layoutConversation: (expanded: number[], selected: number | null) => void;
  selectMessage: (id: number | null, unfold?: number[]) => void;
  /** Expands or collapses a message; a click also selects it. */
  toggleExpanded: (id: number, select: boolean) => void;
  unfold: (ids: number[]) => void;
  toggleExpandAll: () => void;
  setLink: (link: [number, number] | null) => void;
};

export const defaultChrome = (): AccountChrome => ({
  where: { kind: "view", view: { kind: "inbox" } },
  highlight: null,
  screen: { kind: "list" },
});

const INBOX: Where = { kind: "view", view: { kind: "inbox" } };

/** Adds `x` if missing, else removes it. */
function toggled<T>(xs: readonly T[], x: T): T[] {
  const set = new Set(xs);
  if (!set.delete(x)) set.add(x);
  return [...set];
}

/** Fresh view state for a conversation that opens. */
const conversationReset = {
  selectedMessage: null,
  expanded: [],
  unfolded: [],
  expandAll: false,
  link: null,
};

export const useChrome = create<ChromeState & ChromeActions>((set, get) => {
  /** Patches the account on screen's chrome; a new `where` drops a forced split. */
  const patchCurrent = (patch: Partial<AccountChrome>, more: Partial<ChromeState> = {}) =>
    set((s) => {
      if (s.account === null) return more;
      const cur = s.perAccount[s.account] ?? defaultChrome();
      const perAccount = { ...s.perAccount, [s.account]: { ...cur, ...patch } };
      return patch.where ? { ...more, perAccount, forcedSplit: null } : { ...more, perAccount };
    });

  return {
    account: null,
    perAccount: {},
    selection: [],
    palette: null,
    sheet: null,
    search: null,
    beforeSearch: null,
    ...conversationReset,
    forcedSplit: null,

    current: () => {
      const s = get();
      return s.account === null ? null : (s.perAccount[s.account] ?? null);
    },
    setAccount: (account) =>
      set((s) => {
        let perAccount = s.perAccount;
        // A switch that closes search leaves the old account on the view search replaced.
        const left = s.account === null ? undefined : perAccount[s.account];
        if (s.account !== null && left && s.search && s.beforeSearch) {
          perAccount = { ...perAccount, [s.account]: { ...left, ...s.beforeSearch } };
        }
        if (account !== null && !perAccount[account]) {
          perAccount = { ...perAccount, [account]: defaultChrome() };
        }
        return {
          account,
          perAccount,
          selection: [],
          palette: null,
          search: null,
          beforeSearch: null,
          link: null,
        };
      }),
    showList: (where) =>
      patchCurrent(
        { where, highlight: null, screen: { kind: "list" } },
        where.kind === "search"
          ? { selection: [], link: null }
          : { selection: [], link: null, search: null, beforeSearch: null },
      ),
    leaveConversation: () =>
      patchCurrent({ screen: { kind: "list" } }, { link: null, selectedMessage: null }),
    showConversation: (thread, trashMode, row) =>
      patchCurrent(
        { screen: { kind: "conversation", thread, trashMode }, highlight: row },
        { ...conversationReset, selection: [] },
      ),
    setHighlight: (highlight) => patchCurrent({ highlight }),
    setSelection: (selection) => set({ selection }),
    forceSplit: (forcedSplit) => set({ forcedSplit }),

    openSearch: () =>
      set((s) => {
        if (s.search) return { search: { ...s.search, focus: "field" } };
        const cur = s.account === null ? undefined : s.perAccount[s.account];
        return {
          search: { query: "", focus: "field" },
          beforeSearch: cur ? { where: cur.where, highlight: cur.highlight } : null,
        };
      }),
    setSearchQuery: (query) => {
      const { beforeSearch } = get();
      if (query.trim()) {
        patchCurrent(
          { where: { kind: "search", query }, highlight: null, screen: { kind: "list" } },
          { search: { query, focus: "field" } },
        );
      } else {
        // An emptied field shows the view search replaced.
        patchCurrent(beforeSearch ?? {}, { search: { query, focus: "field" } });
      }
    },
    setSearchFocus: (focus) => set((s) => (s.search ? { search: { ...s.search, focus } } : {})),
    closeSearch: () => {
      const before = get().beforeSearch ?? { where: INBOX, highlight: null };
      patchCurrent(
        { ...before, screen: { kind: "list" } },
        { search: null, beforeSearch: null, selection: [], link: null, selectedMessage: null },
      );
    },

    openPalette: (palette) => set({ palette }),
    closePalette: () => set({ palette: null }),
    openSheet: (sheet) => set({ sheet, palette: null }),
    closeSheet: () => set({ sheet: null }),

    layoutConversation: (expanded, selectedMessage) => set({ expanded, selectedMessage }),
    selectMessage: (selectedMessage, unfold) =>
      set((s) => ({
        selectedMessage,
        unfolded: unfold && unfold.length > 0 ? [...s.unfolded, ...unfold] : s.unfolded,
      })),
    toggleExpanded: (id, select) =>
      set((s) => ({
        expanded: toggled(s.expanded, id),
        selectedMessage: select ? id : s.selectedMessage,
      })),
    unfold: (ids) => set((s) => ({ unfolded: [...s.unfolded, ...ids] })),
    toggleExpandAll: () => set((s) => ({ expandAll: !s.expandAll })),
    setLink: (link) => set({ link }),
  };
});

export const viewTitle = (view: View, labelName?: string): string => {
  switch (view.kind) {
    case "inbox":
      return "Inbox";
    case "done":
      return "Done";
    case "allMail":
      return "All Mail";
    case "snoozed":
      return "Snoozed";
    case "starred":
      return "Starred";
    case "sent":
      return "Sent";
    case "drafts":
      return "Drafts";
    case "trash":
      return "Trash";
    case "spam":
      return "Spam";
    case "label":
      return labelName ?? view.label;
  }
};
