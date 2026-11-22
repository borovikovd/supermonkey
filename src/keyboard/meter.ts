import { launch, perfReport } from "../ipc/bridge";

const BUDGETS: Record<string, number> = {
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

export function measure(action: string, start: number) {
  if (!launch.debug) return;
  const budget = BUDGETS[action];
  if (budget === undefined) return;
  requestAnimationFrame(() => {
    setTimeout(() => {
      const ms = performance.now() - start;
      if (ms > budget) console.warn(`[perf] ${action} ${ms.toFixed(1)}ms > ${budget}ms`);
      void perfReport(action, ms, budget);
    }, 0);
  });
}
