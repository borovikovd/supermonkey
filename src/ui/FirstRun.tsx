import { useState } from "react";

import { addAccount } from "../ipc/bridge";

export function FirstRun() {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  return (
    <div
      data-tauri-drag-region
      className="flex h-full flex-col items-center justify-center gap-4 bg-surface"
    >
      <div className="text-subject font-semibold">Ubermensch</div>
      <div className="text-row text-text-2">A fast, keyboard-first client for Gmail.</div>
      <button
        disabled={busy}
        className="mt-2 rounded-control bg-accent px-4 py-2 text-row font-medium text-white disabled:opacity-60"
        onClick={() => {
          setBusy(true);
          setError(null);
          addAccount()
            .catch((e: unknown) => setError(String(e)))
            .finally(() => setBusy(false));
        }}
      >
        {busy ? "Waiting for Google in your browser…" : "Add a Gmail account"}
      </button>
      {error && <div className="max-w-md text-center text-row text-danger">{error}</div>}
    </div>
  );
}
