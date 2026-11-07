// The window's keydown handler (SPEC Keyboard): keys that work everywhere,
// then, unless the palette, a field, a sheet, or a composer has the key, the
// shortcut table. Both are looked up in the command table.

import { typeAhead } from "../compose/start";
import { closeSheet } from "../navigation";
import { useChrome } from "../store/chrome";
import { context, runKey } from "./table";

const isTyping = (el: EventTarget | null) => {
  if (!(el instanceof HTMLElement)) return false;
  return (
    el.isContentEditable ||
    el.tagName === "INPUT" ||
    el.tagName === "TEXTAREA" ||
    el.tagName === "SELECT"
  );
};

export function onKeyDown(e: KeyboardEvent) {
  const start = performance.now();
  const c = context();
  if (c.account === null) return;
  // Account switching and Settings work everywhere, even while typing.
  if (runKey("global", e, c, start)) return;
  // Typed after r, f, or Enter, before the reply editor has focus: the reply's text.
  if (typeAhead(e)) return;
  const { palette, sheet } = useChrome.getState();
  // Owned by the palette, a field, or the composer.
  if (palette || isTyping(e.target) || c.composer !== null) return;
  if (sheet) {
    if (e.key === "Escape") {
      e.preventDefault();
      closeSheet();
    }
    return;
  }
  runKey("mail", e, c, start);
}
