// Undo on a send (SPEC Send and undo).

import { act } from "../ipc/bridge";
import type { OutboxRow } from "../ipc/types";

/**
 * Sends undone while waiting, by outbox id. SendToasts reopens the composer
 * once the row is gone from the outbox, if it was still waiting when last
 * seen: Rust cancelled it. A row seen sending or sent before it went was held
 * or trashed instead, and only a notice brings its draft back.
 */
export const reopenWhenGone = new Map<
  number,
  { account: number; draft: string; thread: number | null; waiting: boolean }
>();

export function undoSend(row: OutboxRow) {
  if (row.undoEntry === null) return;
  if (row.state === "waiting") {
    reopenWhenGone.set(row.id, {
      account: row.account,
      draft: row.draft,
      thread: row.thread,
      waiting: true,
    });
  }
  act({ type: "undo", account: row.account, entry: row.undoEntry });
}
