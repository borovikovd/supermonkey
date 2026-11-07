// Undo on a send, and the draft coming back to a composer (SPEC Send and undo).

import { toast } from "sonner";

import { act } from "../ipc/bridge";
import type { Notice, OutboxRow } from "../ipc/types";
import { accountSummary } from "../store/mirror-core";
import { typingInComposer } from "./session";
import { reopenDraft } from "./start";

/**
 * Undoes a send. Rust cancels a send still waiting when the undo reaches it,
 * and its draft comes back in a composer. One already sending holds the
 * undo, and its notice brings the draft back if it was not sent; a sent one
 * is trashed. A refused undo says why.
 */
export function undoSend(row: OutboxRow) {
  if (row.undoEntry === null) return;
  act({ type: "undo", account: row.account, entry: row.undoEntry }, ({ refusal, before }) => {
    if (refusal) {
      toast.error(refusal.message);
      return;
    }
    // The row as Rust had it when it applied the undo.
    const was = accountSummary(before, row.account)?.outbox.find((r) => r.id === row.id);
    if (was?.state === "waiting" && !was.undoRequested) {
      reopenDraft(row.account, row.draft, row.thread);
    }
  });
}

/** A send's notice: Gmail refused it, or a held undo resolved as not sent. */
export const isSendNotice = (n: Notice) => n.kind === "sendFailed" || n.kind === "sendUndone";

/**
 * A send that did not go out: refused by Rust or Gmail (`failed` says why),
 * or a held undo that resolved as not sent. Its draft goes back in a
 * composer, unless the user is typing in one: then it stays put, and a toast
 * offers it until acted on or closed, since it is otherwise only in Drafts.
 */
export function notSent(
  account: number,
  draft: string,
  thread: number | null,
  failed: string | null,
) {
  if (failed !== null) toast.error(failed);
  if (!typingInComposer()) {
    reopenDraft(account, draft, thread);
    return;
  }
  toast(failed !== null ? "Your message was not sent" : "Send undone", {
    duration: Infinity,
    closeButton: true,
    action: { label: "Open draft", onClick: () => reopenDraft(account, draft, thread) },
  });
}
