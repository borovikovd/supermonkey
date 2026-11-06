import { toast } from "sonner";
import { create } from "zustand";

import { addAccount, signInAgain } from "../ipc/bridge";
import { useChrome } from "../store/chrome";

/** Shows why a sign-in failed. One replaced by a newer attempt says nothing. */
export function reportSignIn(signIn: Promise<unknown>) {
  signIn.catch((e: unknown) => {
    const message = String(e);
    if (!message.includes("Sign-in cancelled")) toast.error(message);
  });
}

type Status = { busy: boolean; error: string | null };

const IDLE: Status = { busy: false, error: null };

/** Sign-ins started from a screen, by key: `add`, or `again-<account>`. */
const useStatus = create<Record<string, Status>>(() => ({}));
const attempts = new Map<string, number>();

/**
 * Starts a sign-in whose status shows on its screen. Starting again while one
 * waits starts over: the core cancels the sign-in left in the browser, and
 * only the latest attempt reports back.
 */
function startSignIn(key: string, run: () => Promise<unknown>) {
  const mine = (attempts.get(key) ?? 0) + 1;
  attempts.set(key, mine);
  const latest = () => attempts.get(key) === mine;
  useStatus.setState({ [key]: { busy: true, error: null } });
  run()
    .catch((e: unknown) => {
      if (latest())
        useStatus.setState((s) => ({ [key]: { ...(s[key] ?? IDLE), error: String(e) } }));
    })
    .finally(() => {
      if (latest()) useStatus.setState((s) => ({ [key]: { ...(s[key] ?? IDLE), busy: false } }));
    });
}

const againKey = (account: number) => `again-${account}`;

/** The first-run screen's Add a Gmail account. */
export const startAddAccount = () => startSignIn("add", addAccount);

export const useAddAccountStatus = () => useStatus((s) => s.add ?? IDLE);

/** Sign in again for a signed-out account, from its screen's button or `Enter`. */
export function startSignInAgain(account: number) {
  startSignIn(againKey(account), () => signInAgain(account));
}

/** `Enter` on the signed-out account's screen. */
export function signInAgainOnScreen() {
  const { account } = useChrome.getState();
  if (account !== null) startSignInAgain(account);
}

export const useSignInAgainStatus = (account: number) =>
  useStatus((s) => s[againKey(account)] ?? IDLE);
