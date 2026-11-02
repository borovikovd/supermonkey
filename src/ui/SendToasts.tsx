import { useEffect, useMemo, useRef } from "react";
import { toast } from "sonner";

import { act } from "../ipc/bridge";
import type { Notice } from "../ipc/types";
import { useMirror } from "../store/mirror";
import { SendToastList } from "./SendToastList";

/** All notices across accounts. A notice persists until shown and acked. */
function useNotices(): Notice[] {
  const subs = useMirror((s) => s.subs);
  return useMemo(() => {
    const out: Notice[] = [];
    for (const sub of Object.values(subs)) {
      if (sub.type === "account" && sub.summary) out.push(...sub.summary.notices);
    }
    return out;
  }, [subs]);
}

export function SendToasts() {
  const notices = useNotices();
  const shown = useRef(new Set<number>());
  useEffect(() => {
    for (const n of notices) {
      if (shown.current.has(n.id)) continue;
      shown.current.add(n.id);
      if (n.kind === "sendFailed" || n.kind === "sendUndone") {
        // Handled by the composer flow (see compose/notices).
        continue;
      }
      if (n.kind === "draftGone") toast(n.message);
      else if (n.kind === "rejected") toast.error(n.message);
      else toast(n.message);
      act({ type: "ackNotice", notice: n.id });
    }
  }, [notices]);
  return <SendToastList />;
}
