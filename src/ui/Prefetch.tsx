import { useEffect, useState } from "react";

import { visibleIds } from "../commands";
import { subscribe, unsubscribe } from "../ipc/bridge";
import type { SubKey } from "../ipc/types";
import { useChrome } from "../store/chrome";

/**
 * Subscribes the highlighted conversation and its neighbors once the
 * highlight rests for 100 ms (in an open conversation, its neighbors once it
 * has been open 100 ms), so opening one paints from the mirror.
 */
export function Prefetch() {
  const focus = useChrome((s) => {
    const cur = s.account === null ? null : s.perAccount[s.account];
    if (!cur) return null;
    if (cur.screen.kind === "conversation") return `c:${cur.screen.thread}:${cur.screen.trashMode}`;
    return cur.highlight ? `l:${cur.highlight}` : null;
  });
  const [keys, setKeys] = useState<SubKey[]>([]);

  useEffect(() => {
    if (!focus) return;
    const t = setTimeout(() => {
      const ids = visibleIds();
      const [kind, id, tm] = focus.split(":");
      const at = ids.indexOf(id!);
      const trashMode =
        tm === "true" ||
        (() => {
          const c = useChrome.getState().current();
          const v = c?.where.kind === "view" ? c.where.view.kind : null;
          return v === "trash" || v === "spam";
        })();
      const around = [ids[at - 1], kind === "l" ? id : undefined, ids[at + 1]].filter(
        (x): x is string => x !== undefined && !Number.isNaN(Number(x)),
      );
      setKeys(
        around.map((x) => ({ type: "conversation", thread: Number(x), trashMode, open: false })),
      );
    }, 100);
    return () => clearTimeout(t);
  }, [focus]);

  useEffect(() => {
    for (const k of keys) subscribe(k);
    return () => {
      for (const k of keys) unsubscribe(k);
    };
  }, [keys]);
  return null;
}
