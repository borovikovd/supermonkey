import { useRef, useState } from "react";

import { addAccount } from "../ipc/bridge";

export function FirstRun() {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // A click while waiting starts over: the core cancels the sign-in left in
  // the browser, and only the latest attempt reports back.
  const attempt = useRef(0);
  const start = () => {
    const mine = ++attempt.current;
    setBusy(true);
    setError(null);
    addAccount()
      .catch((e: unknown) => {
        if (attempt.current === mine) setError(String(e));
      })
      .finally(() => {
        if (attempt.current === mine) setBusy(false);
      });
  };
  return (
    <div
      data-tauri-drag-region
      className="flex h-full flex-col items-center justify-center gap-4 bg-surface"
    >
      <div className="text-subject font-semibold">Ubermensch</div>
      <div className="text-row text-text-2">A fast, keyboard-first client for Gmail.</div>
      <button
        className="mt-2 rounded-control bg-accent px-4 py-2 text-row font-medium text-white"
        onClick={start}
      >
        Add a Gmail account
      </button>
      {busy && (
        <div className="text-row text-text-2">
          Waiting for Google in your browser… Click again to start over.
        </div>
      )}
      {error && <div className="max-w-md text-center text-row text-danger">{error}</div>}
    </div>
  );
}
