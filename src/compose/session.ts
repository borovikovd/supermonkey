// The composer on screen (SPEC Compose, Drafts, UI state → Actions): which
// draft it holds, whether its editor is blurred, and how it leaves the screen.
// This module is the only writer of that state. The mounted composer
// registers a handle here, so send, discard, and close carry its pending save
// in the same invoke.

import { toast } from "sonner";
import { create } from "zustand";

import { act, forgetSub, subIdFor, subscribe } from "../ipc/bridge";
import type { Addr, DraftSave, DraftState, SubKey } from "../ipc/types";
import { useChrome } from "../store/chrome";
import { useMirror } from "../store/mirror";

/** A save of a composer's fields, with the rev of the draft state they are based on. */
export type UpdateSave = Extract<DraftSave, { type: "update" }>;

export type ComposeField = "to" | "cc" | "bcc" | "subject" | "body";

/** What the mounted composer lends the session: its unsaved edits and its fields. */
export type ComposerHandle = {
  draft: string;
  /** Takes the save of pending fields not yet saved, or null when saved. */
  pending: () => UpdateSave | null;
  /** Recipients that are not valid addresses. */
  invalid: () => string[];
  /** Files still being attached: resolves to whether all of them were; null when none are. */
  uploads: () => Promise<boolean> | null;
  setFrom: (from: Addr) => void;
  /** Opens the file picker. */
  attach: () => void;
  /** Shows the field if hidden and focuses it. */
  focus: (field: ComposeField) => void;
  /** Whether the message editor has focus. */
  editorFocused: () => boolean;
  /** Opens the link input on the editor's selection. */
  openLink: () => void;
  /** Closes the link input if open; true if it was. */
  closeLink: () => boolean;
};

export type ComposeState = {
  /** The composer on screen: a reply composer in conversation `thread`, or the `c` screen when null. */
  onScreen: { draft: string; thread: number | null } | null;
  /** The first Esc blurred the editor; compose is still on screen. */
  blurred: boolean;
};

const useCompose = create<ComposeState>(() => ({ onScreen: null, blurred: false }));

/** Reads compose state in a component. */
export const useComposeState = <T>(select: (s: ComposeState) => T): T => useCompose(select);

/** Which composer is on screen: the reply composer in the open conversation, the `c` screen, or none. */
export type ComposerKind = "inline" | "screen" | null;

/** The composer on screen, by kind: what the screen, its hints, and its keys follow. */
export const composerKind = (s: ComposeState): ComposerKind =>
  s.onScreen === null ? null : s.onScreen.thread === null ? "screen" : "inline";

/** Compose state now, outside React. */
export const composeState = () => useCompose.getState();

/** The composer on screen, outside React. */
export const composerOnScreen = () => useCompose.getState().onScreen;

export const composeBlurred = () => useCompose.getState().blurred;

/** Calls `listener` on every change of compose state; returns the unsubscribe. */
export const onComposeChange = (listener: () => void) => useCompose.subscribe(listener);

let mounted: ComposerHandle | null = null;

/** The mounted composer registers itself; the returned function unregisters it. */
export function registerComposer(handle: ComposerHandle): () => void {
  mounted = handle;
  return () => {
    if (mounted === handle) mounted = null;
  };
}

/** The mounted composer's handle, if it holds `draft` (or any draft when omitted). */
export function mountedComposer(draft?: string): ComposerHandle | null {
  return mounted && (draft === undefined || mounted.draft === draft) ? mounted : null;
}

/** The user is typing in a composer: it is mounted and its editor is not blurred. */
export const typingInComposer = () => mounted !== null && !composeBlurred();

export const composerKey = (draft: string): SubKey => ({ type: "composer", draft });

/** The composer's draft as its subscription last brought it; null before its first load. */
export function loadedDraft(draft: string): DraftState | null {
  for (const sub of Object.values(useMirror.getState().subs)) {
    if (sub.type === "composer" && sub.key.draft === draft) return sub.draft;
  }
  return null;
}

/**
 * Null when the composer can be saved whole now; else resolves once its
 * first load is in and its files are attached, to whether all of them were.
 */
function whenReady(draft: string): Promise<boolean> | null {
  const uploads = mountedComposer(draft)?.uploads() ?? null;
  const loaded = () => subIdFor(composerKey(draft)) === undefined || loadedDraft(draft) !== null;
  const load = loaded()
    ? null
    : new Promise<void>((resolve) => {
        const unsub = useMirror.subscribe(() => {
          if (!loaded()) return;
          unsub();
          resolve();
        });
      });
  if (!uploads && !load) return null;
  return Promise.all([load, uploads]).then(([, ok]) => ok ?? true);
}

/** The draft in the composer on screen, inline or full-screen. */
export const draftOnScreen = (): string | null => composerOnScreen()?.draft ?? null;

/** Opens a composer that replaces the screen; closing it shows the screen beneath again. */
export function openScreen(draft: string) {
  subscribe(composerKey(draft));
  useCompose.setState({ onScreen: { draft, thread: null }, blurred: false });
}

/** Opens the reply composer under the open conversation, with its target message selected. */
export function openInline(draft: string, thread: number, target: number | null) {
  subscribe(composerKey(draft));
  useCompose.setState({ onScreen: { draft, thread }, blurred: false });
  // Whenever a reply composer opens, its target message is selected.
  if (target !== null) useChrome.getState().selectMessage(target);
}

/** The first Esc: the editor loses focus and compose stays on screen. */
export function blurComposer() {
  if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
  useCompose.setState({ blurred: true });
}

/** Focus came back into the composer. */
export function composerFocused() {
  if (composeBlurred()) useCompose.setState({ blurred: false });
}

/** Closes that wait for their composer's first load or files, by draft: each resolves once sent. */
const closing = new Map<string, Promise<void>>();

/**
 * Closes the composer on screen as a second Esc would: kept as a draft, or
 * discarded if untouched. A close that waits is in `closing` until sent.
 */
export function closeComposerOnScreen() {
  const draft = draftOnScreen();
  if (!draft) return;
  // This composer's own handle: the close may run after it has left the screen.
  const handle = mountedComposer(draft);
  const ready = whenReady(draft);
  // Off screen at once. The close itself waits for the first load and for
  // files still attaching, or it could discard the draft as untouched.
  leave(draft, false);
  if (!ready) {
    close(draft, handle);
    return;
  }
  const done = (async () => {
    await ready;
    closing.delete(draft);
    close(draft, handle);
  })();
  closing.set(draft, done);
}

/**
 * Cmd+Q: closes the composer on screen, and resolves once every close still
 * waiting for a first load or a file is sent (SPEC UI state, Actions).
 */
export function closeComposers(): Promise<void> {
  closeComposerOnScreen();
  return Promise.all(closing.values()).then(() => undefined);
}

/** Sends a composer's close, with its pending save ahead of it. */
function close(draft: string, handle: ComposerHandle | null) {
  const save = handle?.pending() ?? null;
  if (save) act({ type: "saveDraft", draft, save });
  act({ type: "closeComposer", draft });
  forgetSub(composerKey(draft));
}

/** Takes the composer off screen and, unless told not to, drops its subscription. */
function leave(draft: string, forget = true) {
  if (forget) forgetSub(composerKey(draft));
  if (draftOnScreen() === draft) useCompose.setState({ onScreen: null, blurred: false });
  if (mounted?.draft === draft) mounted = null;
}

/**
 * Cmd+Enter: refuses with a toast while a recipient is invalid. If Rust
 * refuses the send, `refused` gets the draft back (compose/sends.ts, which
 * this module can't import).
 */
export function sendOnScreen(
  refused: (account: number, draft: string, thread: number | null, why: string) => void,
) {
  const draft = draftOnScreen();
  if (!draft) return;
  // Before the first load or while a file is still attaching, the draft is
  // not complete: send once it is, and not at all if a file fails. The
  // composer stays on screen meanwhile.
  const wait = whenReady(draft);
  if (wait) {
    void wait.then((ok) => {
      if (draftOnScreen() !== draft) return;
      if (ok) sendOnScreen(refused);
      else toast.error("Not sent: a file could not be attached");
    });
    return;
  }
  const handle = mountedComposer(draft);
  // Taking the pending save ends it, so it is saved here, before any refusal.
  // It also turns typed text into chips, which the checks below must see.
  const save = handle?.pending() ?? null;
  if (save) act({ type: "saveDraft", draft, save });
  const invalid = handle?.invalid() ?? [];
  if (invalid.length > 0) {
    toast.error(
      `Fix ${invalid.length === 1 ? "this address" : "these addresses"}: ${invalid.join(", ")}`,
    );
    return;
  }
  const recipients = save?.fields ?? loadedDraft(draft);
  if (recipients && recipients.to.length + recipients.cc.length + recipients.bcc.length === 0) {
    toast.error("Add a recipient");
    return;
  }
  const sent = loadedDraft(draft);
  act({ type: "sendDraft", draft }, ({ refusal }) => {
    if (!refusal) return;
    if (sent) refused(sent.account, draft, sent.thread, refusal.message);
    else toast.error(refusal.message);
  });
  leave(draft);
}

/** Cmd+Shift+,: discard locally and on Gmail. */
export function discardOnScreen() {
  const draft = draftOnScreen();
  if (!draft) return;
  act({ type: "discardDraft", draft });
  leave(draft);
  toast("Draft discarded");
}
