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

export function targetsOf(
  open: number | null,
  selection: readonly string[],
  highlight: string | null,
  shown: ReadonlySet<string>,
  drafts: boolean,
): number[] {
  if (open !== null) return [open];
  const selected = selection.filter((k) => shown.has(k));
  if (selected.length > 0) return selected.map(Number).filter((n) => !Number.isNaN(n));
  if (highlight !== null && shown.has(highlight) && !drafts) {
    const n = Number(highlight);
    return Number.isNaN(n) ? [] : [n];
  }
  return [];
}

export function stillShown(threads: number[]): number[] {
  const cur = useChrome.getState().current();
  if (!cur) return [];
  const { screen } = cur;
  if (screen.kind === "conversation") return threads.filter((t) => t === screen.thread);
  const shown = new Set(visibleIds());
  return threads.filter((t) => shown.has(String(t)));
}
