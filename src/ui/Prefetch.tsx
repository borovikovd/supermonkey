import { useEffect, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { visibleIds } from "../commands";
import { later, subscribe, unsubscribe } from "../ipc/bridge";
import type { SubKey } from "../ipc/types";
import { useChrome } from "../store/chrome";

/**
 * Subscribes the highlighted conversation and its neighbors once the
 * highlight rests for 100 ms (in an open conversation, its neighbors once it
 * has been open 100 ms), so opening one paints from the mirror.
 */
export function Prefetch() {
  const { thread, openTrashMode, row } = useChrome(
    useShallow((s) => {
      const cur = s.account === null ? undefined : s.perAccount[s.account];
      return cur?.screen.kind === "conversation"
        ? { thread: cur.screen.thread, openTrashMode: cur.screen.trashMode, row: null }
        : { thread: null, openTrashMode: false, row: cur?.highlight || null };
    }),
  );
  const [keys, setKeys] = useState<SubKey[]>([]);

  useEffect(() => {
    const center = thread === null ? row : String(thread);
    if (center === null) return;
    return later(() => {
      const ids = visibleIds();
      const at = ids.indexOf(center);
      const where = useChrome.getState().current()?.where;
      const view = where?.kind === "view" ? where.view.kind : null;
      const trashMode = openTrashMode || view === "trash" || view === "spam";
      // An open conversation is subscribed already; a highlighted row is not.
      const around = [ids[at - 1], thread === null ? center : undefined, ids[at + 1]].filter(
        (x): x is string => x !== undefined && !Number.isNaN(Number(x)),
      );
      setKeys(
        around.map((x) => ({ type: "conversation", thread: Number(x), trashMode, open: false })),
      );
    }, 100);
  }, [thread, openTrashMode, row]);

  useEffect(() => {
    for (const k of keys) subscribe(k);
    return () => {
      for (const k of keys) unsubscribe(k);
    };
  }, [keys]);
  return null;
}
