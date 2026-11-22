import { toast } from "sonner";
import { create } from "zustand";

import { addAccount, signInAgain } from "../ipc/bridge";
import { useChrome } from "../store/chrome";

function failure(e: unknown): string | null {
  const message = String(e);
  return message.includes("Sign-in cancelled") ? null : message;
}

export function reportSignIn(signIn: Promise<unknown>) {
  signIn.catch((e: unknown) => {
    const message = failure(e);
    if (message !== null) toast.error(message);
  });
}

type Status = { busy: boolean; error: string | null };

const IDLE: Status = { busy: false, error: null };

const useStatus = create<Record<string, Status>>(() => ({}));
const attempts = new Map<string, number>();

function startSignIn(key: string, run: () => Promise<unknown>) {
  const mine = (attempts.get(key) ?? 0) + 1;
  attempts.set(key, mine);
  const latest = () => attempts.get(key) === mine;
  useStatus.setState({ [key]: { busy: true, error: null } });
  run()
    .catch((e: unknown) => {
      if (latest())
        useStatus.setState((s) => ({ [key]: { ...(s[key] ?? IDLE), error: failure(e) } }));
    })
    .finally(() => {
      if (latest()) useStatus.setState((s) => ({ [key]: { ...(s[key] ?? IDLE), busy: false } }));
    });
}

const againKey = (account: number) => `again-${account}`;

export const startAddAccount = () => startSignIn("add", addAccount);

export const useAddAccountStatus = () => useStatus((s) => s.add ?? IDLE);

export function startSignInAgain(account: number) {
  startSignIn(againKey(account), () => signInAgain(account));
}

export function signInAgainOnScreen() {
  const { account } = useChrome.getState();
  if (account !== null) startSignInAgain(account);
}

export const useSignInAgainStatus = (account: number) =>
  useStatus((s) => s[againKey(account)] ?? IDLE);
