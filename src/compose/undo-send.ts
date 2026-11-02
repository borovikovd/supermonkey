// Undo on a send (SPEC Send and undo). For a waiting send, the composer
// reopens once the outbox row is gone; SendToastList watches for that.

import { act } from "../ipc/bridge";
import type { OutboxRow } from "../ipc/types";

/** Undo on a waiting send: the composer opens once the row is gone from the outbox. */
export const reopenWhenGone = new Map<
  number,
  { account: number; draft: string; thread: number | null }
>();

export function undoSend(row: OutboxRow) {
  if (row.undoEntry === null) return;
  if (row.state === "waiting") {
    reopenWhenGone.set(row.id, { account: row.account, draft: row.draft, thread: row.thread });
  }
  act({ type: "undo", account: row.account, entry: row.undoEntry });
}
