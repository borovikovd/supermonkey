import { createRoot } from "react-dom/client";

import "./theme.css";
import { App } from "./App";
import { closeComposers, closeGoneReplies } from "./compose/session";
import {
  connect,
  inTauri,
  onEvent,
  pendingWork,
  quitNow,
  subscribe,
  whenCommitted,
} from "./ipc/bridge";
import { installKeys } from "./keyboard/dispatch";
import { openNotification, trackFocus } from "./navigation";
import { useChrome } from "./store/chrome";
import { useMirror } from "./store/mirror";

if (import.meta.env.DEV || !inTauri)
  Object.assign(window, { __sm: { useChrome, useMirror, pending: pendingWork } });

installKeys();
trackFocus();
closeGoneReplies();

void connect().then(() => subscribe({ type: "app" }));

onEvent("notification-open", ({ account, thread }) => openNotification(account, thread));

const QUIT_WAIT_MS = 5_000;

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
