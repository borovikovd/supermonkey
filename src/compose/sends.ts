// Undo on a send, and the draft coming back to a composer (SPEC Send and undo).

import { act } from "../ipc/bridge";
import type { Notice, OutboxRow } from "../ipc/types";
import { typingInComposer } from "./session";
import { reopenDraft } from "./start";

/**
 * Sends undone while waiting, by outbox id. The composer reopens once the row
 * is gone from the outbox, if it was still waiting when last seen: Rust
 * cancelled it. A row seen sending or sent before it went was held or trashed
 * instead, and only a notice brings its draft back.
 */
const reopenWhenGone = new Map<
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

/** A send's notice: Gmail refused it, or a held undo resolved as not sent. */
export const isSendNotice = (n: Notice) => n.kind === "sendFailed" || n.kind === "sendUndone";

/** Reopens the drafts of sends undone while waiting once their rows leave the outbox. */
export function followOutbox(outbox: OutboxRow[], notices: Notice[]) {
  for (const [id, target] of reopenWhenGone) {
    const row = outbox.find((r) => r.id === id);
    if (row) {
      target.waiting = row.state === "waiting" && !row.undoRequested;
      continue;
    }
    reopenWhenGone.delete(id);
    // A held undo that resolved as not sent comes back through its notice.
    const noticed = notices.some((n) => isSendNotice(n) && n.draft === target.draft);
    if (target.waiting && !noticed) reopenDraft(target.account, target.draft, target.thread);
  }
}

/**
 * A refused send's draft, or one whose held undo resolved as not sent, goes
 * back in a composer, unless the user is typing in one. Then it stays put and
 * this returns false, for the caller to offer it instead.
 */
export function takeBackDraft(account: number, draft: string, thread: number | null): boolean {
  if (typingInComposer()) return false;
  reopenDraft(account, draft, thread);
  return true;
}
