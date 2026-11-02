// Debug builds: keydown-to-paint for each shortcut (SPEC Performance).

import { launch, perfReport } from "../ipc/bridge";

declare global {
  interface Window {
    /** Debug builds: every measured shortcut, for automated checks. */
    ubPerf?: { action: string; ms: number; budget: number }[];
  }
}

export const BUDGETS: Record<string, number> = {
  j: 50,
  k: 50,
  x: 50,
  palette: 50,
  paletteKey: 50,
  autocomplete: 50,
  e: 100,
  "#": 100,
  "!": 100,
  s: 100,
  u: 100,
  z: 100,
  snoozeChoice: 100,
  labelChoice: 100,
  view: 100,
  account: 100,
  open: 100,
  replyInline: 100,
  replyFromList: 150,
  search: 100,
};

/** Call on keydown; measures to the frame that shows the result. */
export function measure(action: string, start = performance.now()) {
  if (!launch.debug) return;
  const budget = BUDGETS[action];
  if (budget === undefined) return;
  // The frame after the next animation frame has been painted.
  requestAnimationFrame(() => {
    setTimeout(() => {
      const ms = performance.now() - start;
      (window.ubPerf ??= []).push({ action, ms, budget });
      if (ms > budget) console.warn(`[perf] ${action} ${ms.toFixed(1)}ms > ${budget}ms`);
      void perfReport(action, ms, budget);
    }, 0);
  });
}
