// Toasts for sends (Sending · Undo, then Sent; SPEC Send and undo) and for
// account notices. They read the account summaries, so they work across
// conversations and account switches.

import { useEffect, useRef } from "react";
import { toast } from "sonner";

import { followOutbox, isSendNotice, takeBackDraft, undoSend } from "../compose/sends";
import { reopenDraft } from "../compose/start";
import { act } from "../ipc/bridge";
import type { Notice, OutboxRow } from "../ipc/types";
import { useMirror } from "../store/mirror";

type AccountNotice = { account: number; notice: Notice };

/** Outbox rows and notices across accounts. A notice persists until shown and acked. */
function useSummaries(): { outbox: OutboxRow[]; notices: AccountNotice[] } {
  const subs = useMirror((s) => s.subs);
  const outbox: OutboxRow[] = [];
  const notices: AccountNotice[] = [];
  for (const sub of Object.values(subs)) {
    if (sub.type !== "account" || !sub.summary) continue;
    const account = sub.summary.account.id;
    outbox.push(...sub.summary.outbox);
    for (const notice of sub.summary.notices) notices.push({ account, notice });
  }
  return { outbox, notices };
}

function sendLabel(r: OutboxRow): string {
  if (r.state === "sent") return "Sent";
  if (r.delayed) return "Email delayed";
  return r.undoRequested ? "Undoing…" : "Sending…";
}

export function SendToasts() {
  const { outbox, notices } = useSummaries();
  const closing = useRef(new Set<number>());
  const shown = useRef(new Set<number>());
  /** Outbox rows with a toast up. */
  const open = useRef(new Set<number>());

  // One toast per outbox row whose toast is still open.
  useEffect(() => {
    const live = new Set(outbox.map((r) => r.id));
    for (const r of outbox) {
      if (r.toastClosed) {
        toast.dismiss(`send-${r.id}`);
        open.current.delete(r.id);
        continue;
      }
      open.current.add(r.id);
      toast(sendLabel(r), {
        id: `send-${r.id}`,
        duration: Infinity,
        description: r.subject || undefined,
        action:
          r.undoEntry !== null && !r.undoRequested
            ? {
                label: "Undo",
                onClick: (e) => {
                  // The toast stays: a held undo shows Undoing…, and a row
                  // that leaves the outbox takes its toast with it.
                  e.preventDefault();
                  undoSend(r);
                },
              }
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
    // A row that left the outbox (undone, or rejected) takes its toast with it.
    for (const id of open.current) {
      if (!live.has(id)) {
        open.current.delete(id);
        toast.dismiss(`send-${id}`);
      }
    }
    followOutbox(
      outbox,
      notices.map((n) => n.notice),
    );
  }, [outbox, notices]);

  // A rejected send, or a held undo that resolved as not sent, puts the draft
  // back in a composer; while the user types in one, its toast offers it.
  useEffect(() => {
    for (const { account, notice: n } of notices) {
      if (!isSendNotice(n) || !n.draft || shown.current.has(n.id)) continue;
      shown.current.add(n.id);
      const { draft, thread } = n;
      if (n.kind === "sendFailed") toast.error(n.message);
      if (!takeBackDraft(account, draft, thread)) {
        // It stays until acted on or closed: the draft is otherwise only in Drafts.
        toast(n.kind === "sendFailed" ? "Your message was not sent" : "Send undone", {
          duration: Infinity,
          closeButton: true,
          action: { label: "Open draft", onClick: () => reopenDraft(account, draft, thread) },
        });
      }
      act({ type: "ackNotice", notice: n.id });
    }
  }, [notices]);

  // Every other notice is a plain toast.
  useEffect(() => {
    for (const { notice: n } of notices) {
      if (isSendNotice(n) || shown.current.has(n.id)) continue;
      shown.current.add(n.id);
      if (n.kind === "rejected") toast.error(n.message);
      else toast(n.message);
      act({ type: "ackNotice", notice: n.id });
    }
  }, [notices]);

  return null;
}
