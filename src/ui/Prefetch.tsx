import { useEffect, useRef } from "react";
import { useShallow } from "zustand/react/shallow";

import { listShowsTrash, visibleIds } from "../store/current";
import { later, subscribe, unsubscribe } from "../ipc/bridge";
import type { SubKey } from "../ipc/types";
import { useChrome } from "../store/chrome";

export function Prefetch() {
  const { thread, openTrashMode, row } = useChrome(
    useShallow((s) => {
      const cur = s.account === null ? undefined : s.perAccount[s.account];
      return cur?.screen.kind === "conversation"
        ? { thread: cur.screen.thread, openTrashMode: cur.screen.trashMode, row: null }
        : { thread: null, openTrashMode: false, row: cur?.highlight || null };
    }),
  );
  const held = useRef<SubKey[]>([]);

  useEffect(() => {
    const center = thread === null ? row : String(thread);
    if (center === null) return;
    return later(() => {
      const ids = visibleIds();
      const at = ids.indexOf(center);
      const trashMode = openTrashMode || listShowsTrash();
      const around = [ids[at - 1], thread === null ? center : undefined, ids[at + 1]].filter(
        (x): x is string => x !== undefined && !Number.isNaN(Number(x)),
      );
      const next: SubKey[] = around.map((x) => ({
        type: "conversation",
        thread: Number(x),
        trashMode,
        open: false,
      }));
      for (const k of next) subscribe(k);
      for (const k of held.current) unsubscribe(k);
      held.current = next;
    }, 100);
  }, [thread, openTrashMode, row]);

  useEffect(
    () => () => {
      for (const k of held.current) unsubscribe(k);
      held.current = [];
    },
    [],
  );
  return null;
}
