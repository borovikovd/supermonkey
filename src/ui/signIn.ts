import { useRef, useState } from "react";
import { toast } from "sonner";

/** Shows why a sign-in failed. One replaced by a newer attempt says nothing. */
export function reportSignIn(signIn: Promise<unknown>) {
  signIn.catch((e: unknown) => {
    const message = String(e);
    if (!message.includes("Sign-in cancelled")) toast.error(message);
  });
}

/**
 * A sign-in started from a button. A click while waiting starts over: the core
 * cancels the sign-in left in the browser, and only the latest attempt reports back.
 */
export function useSignIn(run: () => Promise<unknown>) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const attempt = useRef(0);
  const start = () => {
    const mine = ++attempt.current;
    setBusy(true);
    setError(null);
    run()
      .catch((e: unknown) => {
        if (attempt.current === mine) setError(String(e));
      })
      .finally(() => {
        if (attempt.current === mine) setBusy(false);
      });
  };
  return { busy, error, start };
}
