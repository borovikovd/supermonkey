import { toast } from "sonner";

import { act } from "../ipc/bridge";
import type { Notice, OutboxRow } from "../ipc/types";
import { accountSummary } from "../store/mirror-core";
import { inOverlay, typingInComposer } from "./session";
import { reopenDraft } from "./start";

export function undoSend(row: OutboxRow) {
  if (row.undoEntry === null) return;
  act({ type: "undo", account: row.account, entry: row.undoEntry }, ({ refusal, before }) => {
    if (refusal) {
      toast.error(refusal.message);
      return;
    }
    const was = accountSummary(before, row.account)?.outbox.find((r) => r.id === row.id);
    if (was?.state === "waiting" && !was.undoRequested) {
      reopenDraft(row.account, row.draft, row.thread);
    }
  });
}

export const isSendNotice = (n: Notice) => n.kind === "sendFailed" || n.kind === "sendUndone";

export function notSent(
  account: number,
  draft: string,
  thread: number | null,
  failed: string | null,
) {
  if (failed !== null) toast.error(failed);
  if (!typingInComposer() && !inOverlay()) {
    reopenDraft(account, draft, thread);
    return;
  }
  toast(failed !== null ? "Your message was not sent" : "Send undone", {
    duration: Infinity,
    closeButton: true,
    action: { label: "Open draft", onClick: () => reopenDraft(account, draft, thread) },
  });
}
