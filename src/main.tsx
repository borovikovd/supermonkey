import { createRoot } from "react-dom/client";

import "./theme.css";
import { App } from "./App";
import { useChrome } from "./store/chrome";
import { useMirror } from "./store/mirror";

// Debug builds: the stores, for inspection in the web inspector.
if (import.meta.env.DEV) Object.assign(window, { __ub: { useChrome, useMirror } });

createRoot(document.getElementById("root")!).render(<App />);
