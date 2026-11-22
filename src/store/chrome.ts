import { create } from "zustand";

import type { View } from "../ipc/types";

export type Where = { kind: "view"; view: View } | { kind: "search"; query: string };

type Screen = { kind: "list" } | { kind: "conversation"; thread: number; trashMode: boolean };

export type PaletteMode = (
  | { kind: "commands" }
  | { kind: "labels" }
  | { kind: "goLabel" }
  | { kind: "snooze" }
  | { kind: "unsubscribe" }
  | { kind: "block"; of?: { account: number; thread: number } }
  | { kind: "spam" }
  | { kind: "blocked" }
  | { kind: "switchAccount" }
  | { kind: "addToSplit"; preset?: "important" | "other" }
  | { kind: "from" }
) & {
  threads?: number[];
};

export type Sheet = "shortcuts" | "settings" | "splits" | null;

type AccountChrome = {
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
  beforeSearch: { where: Where; highlight: string | null } | null;
  selectedMessage: number | null;
  expanded: number[];
  unfolded: number[];
  expandAll: boolean;
  link: [number, number] | null;
  forcedSplit: number | null;
};

type ChromeActions = {
  current: () => AccountChrome | null;
  setAccount: (account: number | null) => void;
  showList: (where: Where) => void;
  leaveConversation: () => void;
  showConversation: (thread: number, trashMode: boolean, row: string) => void;
  setHighlight: (key: string | null) => void;
  setSelection: (keys: string[]) => void;
  forceSplit: (split: number | null) => void;

  openSearch: () => void;
  setSearchQuery: (query: string) => void;
  setSearchFocus: (focus: "field" | "results") => void;
  closeSearch: () => void;

  openPalette: (mode: PaletteMode) => void;
  closePalette: () => void;
  openSheet: (sheet: Exclude<Sheet, null>) => void;
  closeSheet: () => void;

  layoutConversation: (expanded: number[], selected: number | null) => void;
  selectMessage: (id: number | null, unfold?: number[]) => void;
  toggleExpanded: (id: number, select: boolean) => void;
  unfold: (ids: number[]) => void;
  toggleExpandAll: () => void;
  setLink: (link: [number, number] | null) => void;
};

const defaultChrome = (): AccountChrome => ({
  where: { kind: "view", view: { kind: "inbox" } },
  highlight: null,
  screen: { kind: "list" },
});

export function toggled<T>(xs: readonly T[], x: T): T[] {
  const set = new Set(xs);
  if (!set.delete(x)) set.add(x);
  return [...set];
}

const conversationReset = {
  selectedMessage: null,
  expanded: [],
  unfolded: [],
  expandAll: false,
  link: null,
};

export const useChrome = create<ChromeState & ChromeActions>((set, get) => {
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
        patchCurrent(beforeSearch ?? {}, { search: { query, focus: "field" } });
      }
    },
    setSearchFocus: (focus) => set((s) => (s.search ? { search: { ...s.search, focus } } : {})),
    closeSearch: () => {
      const before = get().beforeSearch ?? { where: defaultChrome().where, highlight: null };
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
