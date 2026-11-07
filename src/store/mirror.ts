import { create } from "zustand";

import type { ChannelMessage, ListRow } from "../ipc/types";
import { useChrome } from "./chrome";
import {
  addPending,
  applyChannel,
  dropUnheldRows,
  emptyMirror,
  mergeRow,
  type MirrorState,
  type Pending,
  type Sub,
} from "./mirror-core";

type MirrorStore = MirrorState & {
  receive: (msg: ChannelMessage) => void;
  pend: (p: Pending) => void;
  /** Forgets pending actions Rust will never apply: they were never delivered. */
  dropPending: (ns: number[]) => void;
  addSub: (id: string, sub: Sub) => void;
  /** Closes a subscription and drops its rows unless something else holds them. */
  dropSub: (id: string) => void;
  /** Rows fetched outside the pushed window; kept unless ours are newer. */
  putRows: (rows: ListRow[]) => void;
};

/** Rows the selection holds, kept until it clears (SPEC UI state → Actions). */
const selected = () => new Set(useChrome.getState().selection);

export const useMirror = create<MirrorStore>((set) => ({
  ...emptyMirror(),
  receive: (msg) => set((s) => applyChannel(s, msg, selected())),
  pend: (p) => set((s) => addPending(s, p)),
  dropPending: (ns) =>
    set((s) => {
      const drop = new Set(ns);
      return { pending: s.pending.filter((p) => !drop.has(p.n)) };
    }),
  addSub: (id, sub) => set((s) => ({ subs: { ...s.subs, [id]: sub } })),
  dropSub: (id) =>
    set((s) => {
      const { [id]: dropped, ...subs } = s.subs;
      if (dropped?.type !== "list") return { subs };
      const rows = { ...s.rows };
      const keys = dropped.ids.map((e) => e.key);
      dropUnheldRows(rows, subs, s.removals, keys, selected());
      return { subs, rows };
    }),
  putRows: (rows) =>
    set((s) => {
      const next = { ...s.rows };
      for (const r of rows) mergeRow(next, r);
      return { rows: next };
    }),
}));
