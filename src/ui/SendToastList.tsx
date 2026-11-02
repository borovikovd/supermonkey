// Send toasts: Sending · Undo, then Sent (SPEC Send and undo). They read the
// outbox from the account summaries, so they work across conversations and
// account switches.

import { useEffect, useMemo, useRef } from "react";
import { toast } from "sonner";

import { openComposer, reopenDraft } from "../compose/start";
import { reopenWhenGone, undoSend } from "../compose/undo-send";
import { act } from "../ipc/bridge";
import type { Notice, OutboxRow } from "../ipc/types";
import { useChrome } from "../store/chrome";
import { useMirror } from "../store/mirror";

function useOutbox(): OutboxRow[] {
  const subs = useMirror((s) => s.subs);
  return useMemo(() => {
    const out: OutboxRow[] = [];
    for (const sub of Object.values(subs)) {
      if (sub.type === "account" && sub.summary) out.push(...sub.summary.outbox);
    }
    return out;
  }, [subs]);
}

function useNotices(): Notice[] {
  const subs = useMirror((s) => s.subs);
  return useMemo(() => {
    const out: Notice[] = [];
    for (const sub of Object.values(subs)) {
      if (sub.type === "account" && sub.summary) {
        for (const n of sub.summary.notices) out.push({ ...n, thread: n.thread ?? null });
      }
    }
    return out;
  }, [subs]);
}

const typing = () => openComposer.draft !== null && !useChrome.getState().composeBlurred;

export function SendToastList() {
  const rows = useOutbox();
  const notices = useNotices();
  const closing = useRef(new Set<number>());
  const handled = useRef(new Set<number>());

  // One toast per outbox row whose toast is still open.
  useEffect(() => {
    const live = new Set(rows.map((r) => r.id));
    for (const r of rows) {
      if (r.toastClosed) {
        toast.dismiss(`send-${r.id}`);
        continue;
      }
      const label =
        r.state === "sent"
          ? "Sent"
          : r.delayed
            ? "Email delayed"
            : r.undoRequested
              ? "Undoing…"
              : "Sending…";
      toast(label, {
        id: `send-${r.id}`,
        duration: Infinity,
        description: r.subject || undefined,
        action:
          r.undoEntry !== null && !r.undoRequested
            ? { label: "Undo", onClick: () => undoSend(r) }
            : undefined,
        onDismiss: () => act({ type: "closeToast", outbox: r.id }),
      });
      if (r.state === "sent" && !closing.current.has(r.id)) {
        closing.current.add(r.id);
        setTimeout(() => {
          toast.dismiss(`send-${r.id}`);
          act({ type: "closeToast", outbox: r.id });
        }, 4000);
      }
    }
    for (const [id, target] of reopenWhenGone) {
      if (!live.has(id)) {
        reopenWhenGone.delete(id);
        toast.dismiss(`send-${id}`);
        reopenDraft(target.account, target.draft, target.thread);
      }
    }
  }, [rows]);

  // A rejected send, or a held undo that resolved as not sent, puts the draft
  // back in a composer, unless the user is typing in one.
  useEffect(() => {
    for (const n of notices) {
      if (
        handled.current.has(n.id) ||
        (n.kind !== "sendFailed" && n.kind !== "sendUndone") ||
        !n.draft
      )
        continue;
      handled.current.add(n.id);
      const account = Object.values(useMirror.getState().subs).find(
        (s) => s.type === "account" && s.summary?.notices.some((x) => x.id === n.id),
      );
      const accountId = account?.type === "account" ? (account.summary?.account.id ?? null) : null;
      if (accountId === null) continue;
      const draft = n.draft;
      if (n.kind === "sendFailed") toast.error(n.message);
      if (typing()) {
        toast(n.kind === "sendFailed" ? "Your message was not sent" : "Send undone", {
          action: { label: "Open draft", onClick: () => reopenDraft(accountId, draft, n.thread) },
        });
      } else {
        reopenDraft(accountId, draft, n.thread);
      }
      act({ type: "ackNotice", notice: n.id });
    }
  }, [notices]);

  return null;
}
