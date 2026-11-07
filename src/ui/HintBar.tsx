import { hintOf, hints, useCtx } from "../keyboard/table";

/** The hint bar: the commands that work on this screen, from the command table. */
export function HintBar() {
  const items = hints(useCtx()).map(hintOf);
  return (
    <div
      data-hintbar
      className="flex h-hintbar shrink-0 items-center justify-center gap-2 bg-hintbar text-hint font-medium text-text-2"
    >
      {items.map((h, i) => (
        <span key={h} className="flex items-center gap-2">
          {i > 0 && <span className="text-text-3">·</span>}
          {h}
        </span>
      ))}
    </div>
  );
}
