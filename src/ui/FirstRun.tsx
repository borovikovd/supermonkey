import { addAccount } from "../ipc/bridge";
import { useSignIn } from "./signIn";

export function FirstRun() {
  const { busy, error, start } = useSignIn(addAccount);
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
      <SignInStatus busy={busy} error={error} />
    </div>
  );
}

export function SignInStatus({ busy, error }: { busy: boolean; error: string | null }) {
  return (
    <>
      {busy && (
        <div className="text-row text-text-2">
          Waiting for Google in your browser… Click again to start over.
        </div>
      )}
      {error && <div className="max-w-md text-center text-row text-danger">{error}</div>}
    </>
  );
}
