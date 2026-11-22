import { toast } from "sonner";

import type { Action, Rejection } from "../ipc/types";

const BOOKKEEPING = new Set<Action["type"]>([
  "reset",
  "subscribe",
  "unsubscribe",
  "moreResults",
  "ackNotice",
  "closeToast",
]);

export function showRefusal(refusal: Rejection, action: Action | null) {
  if (action === null || BOOKKEEPING.has(action.type)) {
    console.warn(`action ${refusal.n} refused: ${refusal.message}`);
    return;
  }
  toast.error(refusal.message);
}
