// Mail commands shared by the keyboard, the palette, and buttons. Each
// resolves its targets and sends a resolved action (never a toggle); the
// highlight then moves on as SPEC Keyboard says (src/navigation.ts).

import { toast } from "sonner";

import { undoSend } from "./compose/sends";
import { act, blockSender, senderThreads, signInAgain, unsubscribeFrom } from "./ipc/bridge";
import type { ModifyAction, ModifyKind } from "./ipc/types";
import { moveOn, openPalette } from "./navigation";
import { useChrome } from "./store/chrome";
import { currentView, rowOf, visibleIds } from "./store/current";
import { accountSummary, undoTarget, viewLabel } from "./store/mirror-core";
import { useMirror } from "./store/mirror";
import { reportSignIn } from "./ui/signIn";

/**
 * Threads an action applies to: the selection, else the open conversation,
 * else the highlight. On the list only rows still shown count, so a row a
 * poll took away is never acted on.
 */
export function targets(): number[] {
  const c = useChrome.getState();
  const cur = c.current();
  if (!cur) return [];
  if (cur.screen.kind === "conversation") return [cur.screen.thread];
  const shown = new Set(visibleIds());
  const selection = c.selection.filter((k) => shown.has(k));
  if (selection.length > 0) return selection.map(Number).filter((n) => !Number.isNaN(n));
  if (cur.highlight !== null && shown.has(cur.highlight) && currentView()?.kind !== "drafts") {
    const n = Number(cur.highlight);
    return Number.isNaN(n) ? [] : [n];
  }
  return [];
}

export function modify(
  kind: ModifyKind,
  add: string[],
  remove: string[],
  opts: { snooze?: ModifyAction["snooze"]; threads?: number[]; undoable?: boolean } = {},
) {
  const c = useChrome.getState();
  if (c.account === null) return;
  const threads = opts.threads ?? targets();
  if (threads.length === 0) return;
  act({
    type: "modify",
    account: c.account,
    kind,
    threads,
    add,
    remove,
    snooze: opts.snooze ?? null,
    undoable: opts.undoable ?? true,
  });
  if (opts.undoable !== false) c.setSelection([]);
  moveOn(threads);
}

const isSnoozed = (threads: number[]) => threads.some((t) => rowOf(String(t))?.snoozeUntil != null);

/** Archives and marks read, as Superhuman's Mark Done does; z restores both. */
export const markDone = () => {
  if (currentView()?.kind === "done") return;
  modify("done", [], ["INBOX", "UNREAD"], {
    snooze: isSnoozed(targets()) ? { kind: "clear" } : null,
  });
};

export const returnToInbox = () => {
  const view = currentView();
  const remove = view?.kind === "trash" ? ["TRASH"] : view?.kind === "spam" ? ["SPAM"] : [];
  modify("returnToInbox", ["INBOX"], remove, {
    snooze: isSnoozed(targets()) ? { kind: "clear" } : null,
  });
};

export const trash = () => {
  if (currentView()?.kind === "trash") return;
  modify("trash", ["TRASH"], ["INBOX", "SPAM"]);
};

export const spam = () => {
  if (currentView()?.kind === "spam") return;
  modify("spam", ["SPAM"], ["INBOX", "TRASH"]);
};

export const snooze = (at: number) =>
  modify("snooze", [], ["INBOX"], { snooze: { kind: "set", at } });

const allHave = (field: "starred" | "unread") => {
  const t = targets();
  return t.length > 0 && t.every((id) => rowOf(String(id))?.[field]);
};

export const toggleStar = () => {
  if (allHave("starred")) modify("unstar", [], ["STARRED"]);
  else modify("star", ["STARRED"], []);
};

export const toggleRead = () => {
  if (allHave("unread")) modify("read", [], ["UNREAD"]);
  else modify("unread", ["UNREAD"], []);
};

/** `l`: if every target has the label it is removed from all, else added to all. */
export function applyLabel(labelId: string) {
  const t = targets();
  const all = t.length > 0 && t.every((id) => rowOf(String(id))?.labels.includes(labelId));
  if (all) modify("removeLabel", [], [labelId]);
  else modify("label", [labelId], []);
}

/** `y`: remove the current view's label when it can be removed. */
export function removeViewLabel() {
  const view = currentView();
  // Gmail does not let SENT be removed.
  const label = view && view.kind !== "sent" ? viewLabel(view) : null;
  if (!label) return;
  modify("removeLabel", [], [label]);
}

export function undo() {
  const c = useChrome.getState();
  if (c.account === null) return;
  const entry = undoTarget(useMirror.getState(), c.account);
  if (entry === null) return;
  // `z` on a send: the composer reopens as with the toast's Undo.
  const summary = accountSummary(useMirror.getState(), c.account);
  const outbox = summary?.undo.find((u) => u.id === entry)?.outbox;
  const row = outbox == null ? undefined : summary?.outbox.find((o) => o.id === outbox);
  if (row) {
    undoSend(row);
    return;
  }
  act({ type: "undo", account: c.account, entry });
}

/** What Unsubscribe does to the mail already in the Inbox, as in Superhuman. */
export type UnsubscribeThen = "keep" | "done" | "trash";

/**
 * Unsubscribes from the list the conversation on screen came from. "done"
 * and "trash" first archive or trash every Inbox conversation from that
 * sender, as ordinary undoable actions.
 */
export async function unsubscribe(then: UnsubscribeThen) {
  const account = useChrome.getState().account;
  const [thread] = targets();
  if (account === null || thread === undefined) return;
  if (then !== "keep") {
    const same: number[] = await senderThreads(account, thread).catch((err: unknown) => {
      console.warn("conversations from the same sender", err);
      return [];
    });
    const threads = same.includes(thread) ? same : [thread, ...same];
    if (then === "done") modify("done", [], ["INBOX", "UNREAD"], { threads });
    else modify("trash", ["TRASH"], ["INBOX", "SPAM"], { threads });
  }
  try {
    const r = await unsubscribeFrom(account, thread);
    if (r.kind === "noLink") {
      toast("No unsubscribe link in this email", {
        action: { label: "Block sender", onClick: () => openPalette({ kind: "block" }) },
      });
    } else if (r.kind === "openedPage") toast(`Opened the unsubscribe page of ${r.sender}`);
    else toast(`Unsubscribed from ${r.sender}`);
  } catch (err: unknown) {
    toast.error(`Couldn't unsubscribe: ${String(err)}`);
  }
}

/**
 * Blocks the sender (or its domain) of `thread` with a Gmail filter that
 * sends its mail to Trash, then trashes the conversation. With `asSpam`, the
 * conversation is marked spam first instead (Superhuman's "Mark spam, and
 * block"). An account signed in before filters were asked for is offered
 * Sign in again.
 */
export async function block(thread: number, domain: boolean, asSpam = false) {
  const account = useChrome.getState().account;
  if (account === null) return;
  if (asSpam) modify("spam", ["SPAM"], ["INBOX", "TRASH"], { threads: [thread] });
  try {
    const r = await blockSender(account, thread, domain);
    if (r.kind === "needsPermission") {
      toast("Blocking needs one more Google permission", {
        action: { label: "Sign in again", onClick: () => reportSignIn(signInAgain(account)) },
      });
      return;
    }
    if (!asSpam) modify("trash", ["TRASH"], ["INBOX", "SPAM"], { threads: [thread] });
    toast(`Blocked ${r.from}`);
  } catch (err: unknown) {
    toast.error(`Couldn't block: ${String(err)}`);
  }
}
