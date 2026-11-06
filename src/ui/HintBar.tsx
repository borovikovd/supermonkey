import { useComposeState } from "../compose/session";
import { useChrome } from "../store/chrome";
import { useNeedsSignin } from "../store/hooks";

export function HintBar() {
  const screen = useChrome((s) =>
    s.account === null ? null : s.perAccount[s.account]?.screen.kind,
  );
  const composing = useComposeState((s) => s.onScreen !== null);
  const signedOut = useNeedsSignin(useChrome((s) => s.account));
  const typingSearch = useChrome((s) => s.search?.focus === "field");
  const drafts = useChrome((s) => {
    const where = s.account === null ? undefined : s.perAccount[s.account]?.where;
    return where?.kind === "view" && where.view.kind === "drafts";
  });
  const hints = composing
    ? ["⌘Enter to Send", "Esc to close", "⌘K for commands"]
    : signedOut
      ? ["Enter to sign in", "⌘K for commands"]
      : typingSearch
        ? ["Enter to see results", "Esc to close search"]
        : screen === "conversation"
          ? ["E to Mark Done", "R to Reply", "Enter to Reply all", "⌘K for commands"]
          : drafts
            ? // Mail actions do nothing in Drafts.
              ["Enter to open draft", "C to compose", "⌘K for commands"]
            : ["E to Mark Done", "H to snooze", "⌘K for commands"];
  return (
    <div className="flex h-hintbar shrink-0 items-center justify-center gap-2 bg-hintbar text-hint font-medium text-text-2">
      {hints.map((h, i) => (
        <span key={h} className="flex items-center gap-2">
          {i > 0 && <span className="text-text-3">·</span>}
          {h}
        </span>
      ))}
    </div>
  );
}
