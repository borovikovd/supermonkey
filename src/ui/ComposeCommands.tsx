// Palette items for a blurred composer (send, attach, discard) and the From picker.

import { discardOnScreen, openComposer, sendOnScreen } from "../compose/start";
import { useChrome } from "../store/chrome";
import { useMirror } from "../store/mirror";

type Item = { id: string; label: string; keys?: string; run: () => void; keep?: boolean };

export function draftPaletteItems(fromForDraft?: string): Item[] {
  if (fromForDraft) {
    const account = useChrome.getState().account;
    const app = Object.values(useMirror.getState().subs).find((s) => s.type === "app");
    const info =
      app?.type === "app" ? app.state?.accounts.find((a) => a.id === account) : undefined;
    return (info?.sendAs ?? []).map((s) => ({
      id: s.email,
      label: s.name ? `${s.name} <${s.email}>` : s.email,
      run: () => openComposer.setFrom({ name: s.name || null, email: s.email }),
    }));
  }
  return [
    { id: "send", label: "Send", keys: "⌘ ↵", run: sendOnScreen },
    { id: "attach", label: "Attach a file", keys: "⌘ ⇧ A", run: () => openComposer.attach() },
    { id: "discard", label: "Discard draft", keys: "⌘ ⇧ ,", run: discardOnScreen },
  ];
}
