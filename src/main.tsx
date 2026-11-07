import { createRoot } from "react-dom/client";

import "./theme.css";
import { App } from "./App";
import { closeComposerOnScreen } from "./compose/session";
import {
  connect,
  inTauri,
  onEvent,
  onRefusal,
  pendingWork,
  quitNow,
  subscribe,
  whenCommitted,
} from "./ipc/bridge";
import { installKeys } from "./keyboard/dispatch";
import { openNotification, trackFocus } from "./navigation";
import { showRefusal } from "./ui/refusals";
import { useChrome } from "./store/chrome";
import { useMirror } from "./store/mirror";

// Debug builds and the web bridge: the stores, for inspection and the E2E
// harness. `pending` is what the UI still owes the core (see `settle()`).
if (import.meta.env.DEV || !inTauri)
  Object.assign(window, { __ub: { useChrome, useMirror, pending: pendingWork } });

installKeys();
trackFocus();
onRefusal(showRefusal);

// App renders once the app state, subscribed after connecting, arrives.
void connect().then(() => subscribe({ type: "app" }));

onEvent("notification-open", ({ account, thread }) => openNotification(account, thread));
// Cmd+Q: close an open composer as a second Esc would, then quit once it commits.
onEvent("quit-requested", () => {
  closeComposerOnScreen();
  void whenCommitted(5_000).then(quitNow);
});

const root = document.querySelector("#root");
if (!root) throw new Error("index.html has no #root element");
createRoot(root).render(<App />);
