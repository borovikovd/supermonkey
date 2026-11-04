import { useEffect } from "react";

import { signInAgain } from "../ipc/bridge";
import { appHooks } from "../keyboard/refs";
import { SignInStatus } from "./FirstRun";
import { useSignIn } from "./signIn";

/** In place of a signed-out account's mail: it looks signed out until it signs in again. */
export function SignInAgain({ account, email }: { account: number; email: string }) {
  const { busy, error, start } = useSignIn(() => signInAgain(account));
  useEffect(() => {
    appHooks.signInAgain = start;
    return () => {
      appHooks.signInAgain = () => {};
    };
  });
  return (
    <div className="flex h-full flex-col items-center justify-center gap-4 px-8 text-center">
      <div className="text-subject">Sign in again</div>
      <div className="max-w-md text-row text-text-2">
        You&apos;re signed out of <span className="text-text">{email}</span>. Sign in to see and
        sync its mail. Anything you sent or changed meanwhile goes out once you are back.
      </div>
      <button
        className="mt-2 rounded-control bg-accent px-4 py-2 text-row font-medium text-white"
        onClick={start}
      >
        Sign in with Google
      </button>
      <SignInStatus busy={busy} error={error} />
    </div>
  );
}
