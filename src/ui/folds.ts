import type { MessageSummary } from "../ipc/types";

export type Block = { type: "message"; m: MessageSummary } | { type: "fold"; ids: number[] };

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
