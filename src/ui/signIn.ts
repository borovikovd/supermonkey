import { toast } from "sonner";

/** Shows why a sign-in failed. One replaced by a newer attempt says nothing. */
export function reportSignIn(signIn: Promise<unknown>) {
  signIn.catch((e: unknown) => {
    const message = String(e);
    if (!message.includes("Sign-in cancelled")) toast.error(message);
  });
}
