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
import type { ListRow, ModifyAction, ModifyKind } from "./ipc/types";
import { moveOn, openPalette } from "./navigation";
import { fixedTargets, stillShown, targetsOf } from "./acting";
import { useChrome } from "./store/chrome";
import { currentView, rowOf, visibleIds } from "./store/current";
import { accountSummary, undoTarget, viewLabel } from "./store/mirror-core";
import { useMirror } from "./store/mirror";
import { reportSignIn } from "./ui/signIn";

export function targets(): number[] {
  const bound = fixedTargets();
  if (bound) return stillShown(bound);
  const c = useChrome.getState();
  const cur = c.current();
  if (!cur) return [];
  return targetsOf(
    cur.screen.kind === "conversation" ? cur.screen.thread : null,
    c.selection,
    cur.highlight,
    new Set(visibleIds()),
    currentView()?.kind === "drafts",
  );
}

export function modify(
  kind: ModifyKind,
  add: string[],
  remove: string[],
  opts: {
    snooze?: ModifyAction["snooze"];
    account?: number;
    threads?: number[];
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
    undoable: true,
  });
  if (account !== c.account) return;
  c.setSelection([]);
  moveOn(threads);
}

type Acting = { account: number; threads: number[] };

function withRows(resolve: (acting: Acting) => void) {
  const { account } = useChrome.getState();
  const threads = targets();
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

export const markDone = () => {
  if (currentView()?.kind === "done") return;
  modify("done", [], ["INBOX", "UNREAD"]);
};

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

export const spam = () => {
  if (currentView()?.kind === "spam") return;
  modify("spam", ["SPAM"], ["INBOX", "TRASH"]);
};

export const snooze = (at: number) =>
  modify("snooze", [], ["INBOX"], { snooze: { kind: "set", at } });

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

export const allLabeled = (rows: readonly (ListRow | undefined)[], label: string) =>
  rows.length > 0 && rows.every((row) => row?.labels.includes(label));

export function applyLabel(labelId: string) {
  withRows((acting) => {
    const all = allLabeled(
      acting.threads.map((id) => rowOf(String(id))),
      labelId,
    );
    if (all) modify("removeLabel", [], [labelId], acting);
    else modify("label", [labelId], [], acting);
  });
}

export function removeViewLabel() {
  const view = currentView();
  const label = view && view.kind !== "sent" ? viewLabel(view) : null;
  if (!label) return;
  modify("removeLabel", [], [label]);
}

export function undo() {
  const c = useChrome.getState();
  if (c.account === null) return;
  const entry = undoTarget(useMirror.getState(), c.account);
  if (entry === null) return;
  const summary = accountSummary(useMirror.getState(), c.account);
  const outbox = summary?.undo.find((u) => u.id === entry)?.outbox;
  const row = outbox == null ? undefined : summary?.outbox.find((o) => o.id === outbox);
  if (row) {
    undoSend(row);
    return;
  }
  act({ type: "undo", account: c.account, entry });
}

type UnsubscribeThen = "keep" | "done" | "trash";

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
