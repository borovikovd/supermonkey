import { useChrome } from "../store/chrome";

export function HintBar() {
  const screen = useChrome((s) =>
    s.account === null ? null : s.perAccount[s.account]?.screen.kind,
  );
  const inline = useChrome((s) => s.inline !== null);
  const hints =
    screen === "compose" || inline
      ? ["⌘Enter to Send", "Esc to close", "⌘K for commands"]
      : screen === "conversation"
        ? ["E to Mark Done", "R to Reply", "Enter to Reply all", "⌘K for commands"]
        : ["E to Mark Done", "H to snooze", "⌘K for commands"];
  return (
    <div className="flex h-hintbar shrink-0 items-center justify-center gap-2 bg-hint text-hint font-medium text-text-2">
      {hints.map((h, i) => (
        <span key={h} className="flex items-center gap-2">
          {i > 0 && <span className="text-text-3">·</span>}
          {h}
        </span>
      ))}
    </div>
  );
}
