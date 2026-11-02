import { listen } from "@tauri-apps/api/event";
import { createRoot } from "react-dom/client";

import "./theme.css";
import { switchAccount } from "./accounts";
import { App } from "./App";
import { goTo } from "./commands";
import { closeComposerOnScreen } from "./compose/start";
import { connect, inTauri, quitNow, subscribe } from "./ipc/bridge";
import { onKeyDown } from "./keyboard/keys";
import { appHooks } from "./keyboard/refs";
import { useChrome } from "./store/chrome";
import { accountSummary, appState } from "./store/mirror-core";
import { useMirror } from "./store/mirror";

// Debug builds: the stores, for inspection in the web inspector.
if (import.meta.env.DEV) Object.assign(window, { __ub: { useChrome, useMirror } });

appHooks.switchAccount = (index) => {
  const a = appState(useMirror.getState())?.accounts[index];
  if (a) switchAccount(a.id);
};
appHooks.splitIdOf = (kind) => {
  const { account } = useChrome.getState();
  if (account === null) return null;
  const summary = accountSummary(useMirror.getState(), account);
  return summary?.splits.find((s) => s.kind === kind && s.enabled)?.id ?? null;
};
appHooks.openPalette = (mode) => useChrome.getState().set({ palette: mode });

window.addEventListener("keydown", onKeyDown);

// App renders once the app state, subscribed after connecting, arrives.
void connect().then(() => subscribe({ type: "app" }));

if (inTauri) {
  // A notification click: that account, Inbox, the thread selected.
  void listen<{ account: number; thread: number | null }>("notification-open", (e) => {
    switchAccount(e.payload.account);
    goTo({ kind: "view", view: { kind: "inbox" } });
    if (e.payload.thread !== null)
      useChrome.getState().patchCurrent({ highlight: String(e.payload.thread) });
  });
  // Cmd+Q: close an open composer as a second Esc would, then quit once it commits.
  void listen("quit-requested", () => {
    closeComposerOnScreen();
    setTimeout(() => void quitNow(), 150);
  });
}

const root = document.querySelector("#root");
if (!root) throw new Error("index.html has no #root element");
createRoot(root).render(<App />);
