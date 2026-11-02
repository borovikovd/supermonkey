// Chrome state: account on screen, each account's last view and highlight,
// selected ids, palette, search, and compose mode (SPEC UI state).

import { create } from "zustand";

import type { View } from "../ipc/types";

export type Where = { kind: "view"; view: View } | { kind: "search"; query: string };

export type Screen =
  | { kind: "list" }
  | { kind: "conversation"; thread: number; trashMode: boolean }
  /** A new message (`c`) or a new-message draft, replacing the screen. */
  | { kind: "compose"; draft: string };

export type PaletteMode =
  | { kind: "commands"; draftActions?: boolean }
  | { kind: "labels" }
  | { kind: "goLabel" }
  | { kind: "snooze" }
  | { kind: "switchAccount" }
  | { kind: "addToSplit"; preset?: "important" | "other" }
  | { kind: "from"; draft: string };

export type Sheet = "shortcuts" | "settings" | "splits" | null;

export type AccountChrome = {
  where: Where;
  highlight: string | null;
  /** Screen to return to when closing a `c` composer. */
  screen: Screen;
};

type ChromeState = {
  account: number | null;
  perAccount: Record<number, AccountChrome>;
  selection: string[];
  palette: PaletteMode | null;
  sheet: Sheet;
  search: { open: boolean; query: string; focus: "field" | "results" } | null;
  /** The previous where, restored when search closes. */
  beforeSearch: Where | null;
  /** The reply composer inside the open conversation. */
  inline: { draft: string; thread: number } | null;
  /** Selected message in the open conversation. */
  selectedMessage: number | null;
  /** Expanded messages in the open conversation (set when it opens). */
  expanded: number[];
  /** Folded runs opened by the user. */
  unfolded: number[];
  expandAll: boolean;
  /** A link reached with Tab inside a message, as [messageId, linkIndex]. */
  link: [number, number] | null;
  /** `g` was pressed; waiting for the second key. */
  gPending: boolean;
  /** The composer editor is blurred (first Esc). */
  composeBlurred: boolean;
  /** A split shown by `g i` / `g o` even though it is empty and hidden. */
  forcedSplit: number | null;
};

type ChromeActions = {
  set: (patch: Partial<ChromeState>) => void;
  setAccount: (account: number | null) => void;
  current: () => AccountChrome | null;
  patchCurrent: (patch: Partial<AccountChrome>) => void;
};

export const defaultChrome = (): AccountChrome => ({
  where: { kind: "view", view: { kind: "inbox" } },
  highlight: null,
  screen: { kind: "list" },
});

export const useChrome = create<ChromeState & ChromeActions>((set, get) => ({
  account: null,
  perAccount: {},
  selection: [],
  palette: null,
  sheet: null,
  search: null,
  beforeSearch: null,
  inline: null,
  selectedMessage: null,
  expanded: [],
  unfolded: [],
  expandAll: false,
  link: null,
  gPending: false,
  composeBlurred: false,
  forcedSplit: null,
  set: (patch) => set(patch),
  setAccount: (account) =>
    set((s) => ({
      account,
      perAccount:
        account !== null && !s.perAccount[account]
          ? { ...s.perAccount, [account]: defaultChrome() }
          : s.perAccount,
      selection: [],
      palette: null,
      search: null,
    })),
  current: () => {
    const s = get();
    return s.account === null ? null : (s.perAccount[s.account] ?? null);
  },
  patchCurrent: (patch) =>
    set((s) => {
      if (s.account === null) return {};
      const cur = s.perAccount[s.account] ?? defaultChrome();
      const perAccount = { ...s.perAccount, [s.account]: { ...cur, ...patch } };
      return patch.where ? { perAccount, forcedSplit: null } : { perAccount };
    }),
}));

export const viewTitle = (view: View, labelName?: string): string => {
  switch (view.kind) {
    case "inbox":
      return "Inbox";
    case "done":
      return "Done";
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
