// Mail commands shared by the keyboard, the palette, and buttons. Each
// resolves its targets and sends a resolved action (never a toggle); the
// highlight then moves on as SPEC Keyboard says (src/navigation.ts).

import { toast } from "sonner";

import { undoSend } from "./compose/sends";
import {
  act,
  blockSender,
  fetchRows,
  senderThreads,
  signInAgain,
  unsubscribeFrom,
} from "./ipc/bridge";
import type { ModifyAction, ModifyKind } from "./ipc/types";
import { moveOn, openPalette } from "./navigation";
import { fixedTargets, stillShown } from "./acting";
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
  const bound = fixedTargets();
  if (bound) return stillShown(bound);
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

/**
 * Sends a mail change. `account` and `threads` name an account and its
 * threads taken earlier, before an await; by default they are the account
 * on screen and the targets now. The screen moves on only when its account
 * is the one acted on.
 */
export function modify(
  kind: ModifyKind,
  add: string[],
  remove: string[],
  opts: {
    snooze?: ModifyAction["snooze"];
    account?: number;
    threads?: number[];
    undoable?: boolean;
  } = {},
) {
  const c = useChrome.getState();
  const account = opts.account ?? c.account;
  if (account === null) return;
  const threads = opts.threads ?? targets();
  if (threads.length === 0) return;
  act({
    type: "modify",
    account,
    kind,
    threads,
    add,
    remove,
    snooze: opts.snooze ?? null,
    undoable: opts.undoable ?? true,
  });
  if (account !== c.account) return;
  if (opts.undoable !== false) c.setSelection([]);
  moveOn(threads);
}

/** The account on screen and the threads an action takes, read before it resolves. */
type Acting = { account: number; threads: number[] };

/**
 * Runs `resolve` on the targets once the mirror has every one's row: a
 * selection can hold threads whose rows the list never loaded (Select all
 * far below the window), and an action resolved against part of them would
 * resolve the wrong way (SPEC UI state, Actions). With every row in, it runs
 * at once.
 */
function withRows(resolve: (acting: Acting) => void, fixed?: number[]) {
  const { account } = useChrome.getState();
  const threads = fixed ? stillShown(fixed) : targets();
  if (account === null || threads.length === 0) return;
  const missing = threads.map(String).filter((k) => !rowOf(k));
  if (missing.length === 0) {
    resolve({ account, threads });
    return;
  }
  fetchRows(false, missing)
    .then((rows) => {
      useMirror.getState().putRows(rows);
      resolve({ account, threads });
    })
    .catch((err: unknown) => {
      toast.error(`Couldn't read the selected conversations: ${String(err)}`);
    });
}

const isSnoozed = (threads: number[]) => threads.some((t) => rowOf(String(t))?.snoozeUntil != null);

/** Archives and marks read, as Superhuman's Mark Done does; z restores both. */
export const markDone = () => {
  if (currentView()?.kind === "done") return;
  withRows(({ account, threads }) =>
    modify("done", [], ["INBOX", "UNREAD"], {
      account,
      threads,
      snooze: isSnoozed(threads) ? { kind: "clear" } : null,
    }),
  );
};

/**
 * `Shift+E`: back to the Inbox. A thread in Trash or Spam leaves it, by its
 * own labels, wherever it was reached: its view, or a search for `in:trash`.
 */
export const returnToInbox = () =>
  withRows(({ account, threads }) => {
    const rows = threads.map((t) => rowOf(String(t)));
    const remove = ["TRASH", "SPAM"].filter((l) => rows.some((r) => r?.labels.includes(l)));
    modify("returnToInbox", ["INBOX"], remove, {
      account,
      threads,
      snooze: isSnoozed(threads) ? { kind: "clear" } : null,
    });
  });

export const trash = () => {
  if (currentView()?.kind === "trash") return;
  modify("trash", ["TRASH"], ["INBOX", "SPAM"]);
};

/** `fixed`: the threads a palette opened on (see `stillShown`). */
export const spam = (fixed?: number[]) => {
  if (currentView()?.kind === "spam") return;
  modify("spam", ["SPAM"], ["INBOX", "TRASH"], fixed ? { threads: stillShown(fixed) } : {});
};

/** `fixed`: the threads a palette opened on (see `stillShown`). */
export const snooze = (at: number, fixed?: number[]) =>
  modify("snooze", [], ["INBOX"], {
    snooze: { kind: "set", at },
    ...(fixed ? { threads: stillShown(fixed) } : {}),
  });

const allHave = (threads: number[], field: "starred" | "unread") =>
  threads.every((id) => rowOf(String(id))?.[field]);

export const toggleStar = () =>
  withRows((acting) => {
    if (allHave(acting.threads, "starred")) modify("unstar", [], ["STARRED"], acting);
    else modify("star", ["STARRED"], [], acting);
  });

export const toggleRead = () =>
  withRows((acting) => {
    if (allHave(acting.threads, "unread")) modify("read", [], ["UNREAD"], acting);
    else modify("unread", ["UNREAD"], [], acting);
  });

/**
 * `l`: if every target has the label it is removed from all, else added to
 * all. `fixed`: the threads the palette opened on (see `stillShown`).
 */
export function applyLabel(labelId: string, fixed?: number[]) {
  withRows((acting) => {
    const all = acting.threads.every((id) => rowOf(String(id))?.labels.includes(labelId));
    if (all) modify("removeLabel", [], [labelId], acting);
    else modify("label", [labelId], [], acting);
  }, fixed);
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
export async function unsubscribe(then: UnsubscribeThen, fixed?: number[]) {
  const account = useChrome.getState().account;
  const [thread] = fixed ? stillShown(fixed) : targets();
  if (account === null || thread === undefined) return;
  if (then !== "keep") {
    const same: number[] = await senderThreads(account, thread).catch((err: unknown) => {
      console.warn("conversations from the same sender", err);
      return [];
    });
    const threads = same.includes(thread) ? same : [thread, ...same];
    if (then === "done") modify("done", [], ["INBOX", "UNREAD"], { account, threads });
    else modify("trash", ["TRASH"], ["INBOX", "SPAM"], { account, threads });
  }
  try {
    const r = await unsubscribeFrom(account, thread);
    if (r.kind === "noLink") {
      toast("No unsubscribe link in this email", {
        action: {
          label: "Block sender",
          onClick: () => openPalette({ kind: "block", of: { account, thread } }),
        },
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
  if (asSpam) modify("spam", ["SPAM"], ["INBOX", "TRASH"], { account, threads: [thread] });
  try {
    const r = await blockSender(account, thread, domain);
    if (r.kind === "needsPermission") {
      toast("Blocking needs one more Google permission", {
        action: { label: "Sign in again", onClick: () => reportSignIn(signInAgain(account)) },
      });
      return;
    }
    if (!asSpam) modify("trash", ["TRASH"], ["INBOX", "SPAM"], { account, threads: [thread] });
    toast(`Blocked ${r.from}`);
  } catch (err: unknown) {
    toast.error(`Couldn't block: ${String(err)}`);
  }
}
