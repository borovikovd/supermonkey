import { createRoot } from "react-dom/client";

import "./theme.css";
import { App } from "./App";
import { closeComposers, closeGoneReplies } from "./compose/session";
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
  Object.assign(window, { __sm: { useChrome, useMirror, pending: pendingWork } });

installKeys();
trackFocus();
closeGoneReplies();
onRefusal(showRefusal);

// App renders once the app state, subscribed after connecting, arrives.
void connect().then(() => subscribe({ type: "app" }));

onEvent("notification-open", ({ account, thread }) => openNotification(account, thread));

/** How long Cmd+Q waits for the page's last actions, in all. */
const QUIT_WAIT_MS = 5_000;

// Cmd+Q: close an open composer as a second Esc would, then quit once every
// close is sent and committed, or after QUIT_WAIT_MS (SPEC UI state, Actions).
onEvent("quit-requested", () => {
  const start = Date.now();
  const bound = new Promise<void>((resolve) => setTimeout(resolve, QUIT_WAIT_MS));
  void Promise.race([closeComposers(), bound])
    .then(() => whenCommitted(Math.max(0, QUIT_WAIT_MS - (Date.now() - start))))
    .then(quitNow);
});

const root = document.querySelector("#root");
if (!root) throw new Error("index.html has no #root element");
createRoot(root).render(<App />);
