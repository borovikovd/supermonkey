import { useChrome } from "./store/chrome";
import { visibleIds } from "./store/current";

let fixed: { threads: number[]; highlight: string | null } | null = null;

export function withFixedTargets<T>(threads: number[], highlight: string | null, run: () => T): T {
  const before = fixed;
  fixed = { threads, highlight };
  try {
    return run();
  } finally {
    fixed = before;
  }
}

export const fixedTargets = (): number[] | null => fixed?.threads ?? null;

export const fixedHighlight = (): string | null => fixed?.highlight ?? null;

export function stillShown(threads: number[]): number[] {
  const cur = useChrome.getState().current();
  if (!cur) return [];
  const { screen } = cur;
  if (screen.kind === "conversation") return threads.filter((t) => t === screen.thread);
  const shown = new Set(visibleIds());
  return threads.filter((t) => shown.has(String(t)));
}
