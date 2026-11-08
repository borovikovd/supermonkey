// What an action acts on when the user chose it earlier than it runs: a
// palette's command acts on the conversations the palette opened on, not on
// where the highlight moved while it was open (AGENTS.md: an action is bound
// to what the user saw).

import { useChrome } from "./store/chrome";
import { visibleIds } from "./store/current";

/** The threads the running palette command is bound to, if one runs. */
let fixed: number[] | null = null;

/**
 * Runs `run` bound to `threads`, fixed when the palette opened: `targets()`
 * gives those still shown, and a palette it opens takes them too.
 */
export function withFixedTargets<T>(threads: number[], run: () => T): T {
  const before = fixed;
  fixed = threads;
  try {
    return run();
  } finally {
    fixed = before;
  }
}

/** The threads the running palette command is bound to, or `null`. */
export const fixedTargets = (): number[] | null => fixed;

/**
 * Of threads fixed when the user acted, the ones still on screen: a row that
 * left meanwhile is not acted on, and the action never falls to the row the
 * highlight moved to.
 */
export function stillShown(threads: number[]): number[] {
  const cur = useChrome.getState().current();
  if (!cur) return [];
  const { screen } = cur;
  if (screen.kind === "conversation") return threads.filter((t) => t === screen.thread);
  const shown = new Set(visibleIds());
  return threads.filter((t) => shown.has(String(t)));
}
