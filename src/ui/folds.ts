import type { MessageSummary } from "../ipc/types";

export type Block = { type: "message"; m: MessageSummary } | { type: "fold"; ids: number[] };

/**
 * Collapsed runs of more than four fold the ones between the first and last
 * into one `N older messages` line (SPEC Reading).
 */
export function blocks(
  messages: MessageSummary[],
  expanded: Set<number>,
  unfolded: Set<number>,
): Block[] {
  const out: Block[] = [];
  let run: MessageSummary[] = [];
  const flush = () => {
    const head = run[0];
    const tail = run.at(-1);
    // Only a message inside the fold opens it; the first and last show anyway.
    const inner = run.slice(1, -1);
    if (run.length > 4 && head && tail && !inner.some((m) => unfolded.has(m.id))) {
      out.push({ type: "message", m: head });
      out.push({ type: "fold", ids: inner.map((m) => m.id) });
      out.push({ type: "message", m: tail });
    } else {
      for (const m of run) out.push({ type: "message", m });
    }
    run = [];
  };
  for (const m of messages) {
    if (expanded.has(m.id)) {
      flush();
      out.push({ type: "message", m });
    } else run.push(m);
  }
  flush();
  return out;
}
