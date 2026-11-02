// Undo on a send (SPEC Send and undo).

import { act } from "../ipc/bridge";
import type { OutboxRow } from "../ipc/types";

/**
 * Waiting sends being undone, by outbox id. SendToasts reopens the composer
 * once the row is gone from the outbox.
 */
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
