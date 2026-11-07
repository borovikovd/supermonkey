// Refusals no screen waits for (SPEC UI state, Actions): an error toast,
// or, for the UI's own bookkeeping, a log line.

import { toast } from "sonner";

import type { Action, Rejection } from "../ipc/types";

/** Actions the UI takes on its own: a refusal of one is a bug, logged and never shown. */
const BOOKKEEPING = new Set<Action["type"]>([
  "reset",
  "subscribe",
  "unsubscribe",
  "moreResults",
  "ackNotice",
  "closeToast",
]);

/** Shows why Rust refused an action this page sent; one an earlier page sent is only logged. */
export function showRefusal(refusal: Rejection, action: Action | null) {
  if (action === null || BOOKKEEPING.has(action.type)) {
    console.warn(`action ${refusal.n} refused: ${refusal.message}`);
    return;
  }
  toast.error(refusal.message);
}
