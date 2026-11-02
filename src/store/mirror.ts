import { create } from "zustand";

import type { ChannelMessage, ListRow } from "../ipc/types";
import {
  addPending,
  applyChannel,
  emptyMirror,
  mergeRow,
  type MirrorState,
  type Pending,
  type Sub,
} from "./mirror-core";

type MirrorStore = MirrorState & {
  receive: (msg: ChannelMessage) => void;
  pend: (p: Pending) => void;
  addSub: (id: string, sub: Sub) => void;
  dropSub: (id: string) => void;
  /** Rows fetched outside the pushed window; kept unless ours are newer. */
  putRows: (rows: ListRow[]) => void;
};

export const useMirror = create<MirrorStore>((set) => ({
  ...emptyMirror(),
  receive: (msg) => set((s) => applyChannel(s, msg)),
  pend: (p) => set((s) => addPending(s, p)),
  addSub: (id, sub) => set((s) => ({ subs: { ...s.subs, [id]: sub } })),
  dropSub: (id) =>
    set((s) => {
      const { [id]: _dropped, ...subs } = s.subs;
      return { subs };
    }),
  putRows: (rows) =>
    set((s) => {
      const next = { ...s.rows };
      for (const r of rows) mergeRow(next, r);
      return { rows: next };
    }),
}));
