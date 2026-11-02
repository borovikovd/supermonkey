// Toasts for sends (Sending · Undo, then Sent; SPEC Send and undo) and for
// account notices. They read the account summaries, so they work across
// conversations and account switches.

import { useEffect, useRef } from "react";
import { toast } from "sonner";

import { openComposer, reopenDraft } from "../compose/start";
import { reopenWhenGone, undoSend } from "../compose/undo-send";
import { act } from "../ipc/bridge";
import type { Notice, OutboxRow } from "../ipc/types";
import { useChrome } from "../store/chrome";
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

const isSendNotice = (n: Notice) => n.kind === "sendFailed" || n.kind === "sendUndone";

const typing = () => openComposer.draft !== null && !useChrome.getState().composeBlurred;

function sendLabel(r: OutboxRow): string {
  if (r.state === "sent") return "Sent";
  if (r.delayed) return "Email delayed";
  return r.undoRequested ? "Undoing…" : "Sending…";
}

export function SendToasts() {
  const { outbox, notices } = useSummaries();
  const closing = useRef(new Set<number>());
  const shown = useRef(new Set<number>());

  // One toast per outbox row whose toast is still open.
  useEffect(() => {
    const live = new Set(outbox.map((r) => r.id));
    for (const r of outbox) {
      if (r.toastClosed) {
        toast.dismiss(`send-${r.id}`);
        continue;
      }
      toast(sendLabel(r), {
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
  }, [outbox]);

  // A rejected send, or a held undo that resolved as not sent, puts the draft
  // back in a composer, unless the user is typing in one.
  useEffect(() => {
    for (const { account, notice: n } of notices) {
      if (!isSendNotice(n) || !n.draft || shown.current.has(n.id)) continue;
      shown.current.add(n.id);
      const { draft, thread } = n;
      if (n.kind === "sendFailed") toast.error(n.message);
      if (typing()) {
        toast(n.kind === "sendFailed" ? "Your message was not sent" : "Send undone", {
          action: { label: "Open draft", onClick: () => reopenDraft(account, draft, thread) },
        });
      } else {
        reopenDraft(account, draft, thread);
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
