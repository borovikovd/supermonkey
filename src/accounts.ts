import { closeComposerOnScreen } from "./compose/start";
import { useChrome } from "./store/chrome";

/** Switching accounts closes the palette and search and an open composer (SPEC Accounts → Switching). */
export function switchAccount(account: number) {
  const c = useChrome.getState();
  if (c.account === account) return;
  closeComposerOnScreen();
  if (c.account !== null) {
    // An open conversation is not reopened; the list is.
    c.patchCurrent({ screen: { kind: "list" } });
  }
  c.setAccount(account);
  c.set({ inline: null, sheet: null, link: null });
  const cur = c.current();
  if (cur && cur.screen.kind !== "list") c.patchCurrent({ screen: { kind: "list" } });
}
