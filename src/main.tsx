import { createRoot } from "react-dom/client";

import "./theme.css";
import { switchAccount } from "./accounts";
import { App } from "./App";
import { goTo } from "./commands";
import { closeComposerOnScreen } from "./compose/start";
import {
  connect,
  inTauri,
  onEvent,
  pendingWork,
  quitNow,
  subscribe,
  whenCommitted,
} from "./ipc/bridge";
import { onKeyDown } from "./keyboard/keys";
import { trackFocus } from "./keyboard/nav";
import { appHooks } from "./keyboard/refs";
import { useChrome } from "./store/chrome";
import { accountSummary, appState } from "./store/mirror-core";
import { useMirror } from "./store/mirror";

// Debug builds and the web bridge: the stores, for inspection and the E2E
// harness. `pending` is what the UI still owes the core (see `settle()`).
if (import.meta.env.DEV || !inTauri)
  Object.assign(window, { __ub: { useChrome, useMirror, pending: pendingWork } });

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
appHooks.closeComposer = closeComposerOnScreen;

window.addEventListener("keydown", onKeyDown);
trackFocus();

// App renders once the app state, subscribed after connecting, arrives.
void connect().then(() => subscribe({ type: "app" }));

// A notification click: that account, Inbox, the thread selected.
onEvent("notification-open", ({ account, thread }) => {
  switchAccount(account);
  goTo({ kind: "view", view: { kind: "inbox" } });
  if (thread !== null) useChrome.getState().patchCurrent({ highlight: String(thread) });
});
// Cmd+Q: close an open composer as a second Esc would, then quit once it commits.
onEvent("quit-requested", () => {
  closeComposerOnScreen();
  void whenCommitted(5_000).then(quitNow);
});

const root = document.querySelector("#root");
if (!root) throw new Error("index.html has no #root element");
createRoot(root).render(<App />);
