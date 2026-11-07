// The composer on screen (SPEC Compose, Drafts, UI state → Actions): which
// draft it holds, whether its editor is blurred, the files attaching to each
// draft, and how a composer leaves the screen. This module is the only
// writer of that state. The mounted composer registers a handle here, so
// send, discard, and close carry its pending save in the same invoke.

import { toast } from "sonner";
import { create } from "zustand";

import {
  act,
  addAttachment,
  forgetSub,
  launch,
  resubscribe,
  subIdFor,
  subscribe,
} from "../ipc/bridge";
import type { Addr, DraftAttachment, DraftSave, DraftState, SubKey } from "../ipc/types";
import { useChrome } from "../store/chrome";
import { conversationData } from "../store/current";
import { useMirror } from "../store/mirror";

/** A save of a composer's fields, with the rev of the draft state they are based on. */
export type UpdateSave = Extract<DraftSave, { type: "update" }>;

export type ComposeField = "to" | "cc" | "bcc" | "subject" | "body";

/** What the mounted composer lends the session: its unsaved edits and its fields. */
export type ComposerHandle = {
  draft: string;
  /** Takes the save of pending fields not yet saved, or null when saved. */
  pending: () => UpdateSave | null;
  /** Turns text typed in To, Cc, and Bcc into chips; only the fields on screen hold it. */
  commitTyped: () => void;
  /** Recipients that are not valid addresses. */
  invalid: () => string[];
  /** Files attached to the draft join the composer's fields. */
  addAttachments: (added: DraftAttachment[]) => void;
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

/** A file still attaching to a draft, shown as a dashed chip. */
export type Attaching = { key: number; name: string };

export type ComposeState = {
  /** The composer on screen: a reply composer in conversation `thread`, or the `c` screen when null. */
  onScreen: { draft: string; thread: number | null } | null;
  /** The first Esc blurred the editor; compose is still on screen. */
  blurred: boolean;
  /** Files still attaching, by draft. */
  attaching: Record<string, Attaching[]>;
};

const useCompose = create<ComposeState>(() => ({ onScreen: null, blurred: false, attaching: {} }));

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

/** Files attached to a draft reopened while they attached, until its composer mounts. */
const owed = new Map<string, DraftAttachment[]>();

/** The mounted composer registers itself; the returned function unregisters it. */
export function registerComposer(handle: ComposerHandle): () => void {
  mounted = handle;
  const due = owed.get(handle.draft);
  if (due) {
    owed.delete(handle.draft);
    handle.addAttachments(due);
  }
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

// ------------------------------------------------------------ attaching files

/** Files still attaching, by draft: each pick resolves to whether all its files were attached. */
const uploads = new Map<string, Set<Promise<boolean>>>();
let attachSeq = 0;
/** Drafts discarded this session: a file that lands for one goes nowhere. Draft ids are never reused. */
const discarded = new Set<string>();

/** A picked file as base64, for the `add_attachment` command. */
async function readFile(file: File): Promise<string> {
  const bytes = new Uint8Array(await file.arrayBuffer());
  let binary = "";
  for (let i = 0; i < bytes.length; i += 0x8000) {
    binary += String.fromCodePoint(...bytes.subarray(i, i + 0x8000));
  }
  return btoa(binary);
}

function setAttaching(draft: string, change: (files: Attaching[]) => Attaching[]) {
  useCompose.setState((s) => {
    const { [draft]: now = [], ...others } = s.attaching;
    const next = change(now);
    return { attaching: next.length > 0 ? { ...others, [draft]: next } : others };
  });
}

/**
 * Attaches files to a draft (SPEC Drafts). A file over the limit is refused
 * before it is read. The files belong to the draft, not to one composer: a
 * close or send waits for them, and each joins the composer that has the
 * draft when it is in, the reopened one too.
 */
export function attachFiles(draft: string, files: File[]) {
  const max = launch.maxAttachmentBytes;
  const fits = files.filter((file) => {
    if (file.size <= max) return true;
    toast.error(
      `Couldn't attach ${file.name}: attachments can be at most ${Math.floor(max / 2 ** 20)} MB`,
    );
    return false;
  });
  if (fits.length === 0) return;
  const picked = fits.map((file) => ({ file, key: ++attachSeq }));
  setAttaching(draft, (a) => [...a, ...picked.map((p) => ({ key: p.key, name: p.file.name }))]);
  const pending = uploads.get(draft) ?? new Set<Promise<boolean>>();
  uploads.set(draft, pending);
  const store = async (): Promise<boolean> => {
    const results = await Promise.all(
      picked.map(async ({ file }): Promise<DraftAttachment | null> => {
        try {
          return await addAttachment(draft, file.name, file.type, await readFile(file));
        } catch (err) {
          toast.error(`Couldn't attach ${file.name}: ${String(err)}`);
          return null;
        }
      }),
    );
    const added = results.filter((a): a is DraftAttachment => a !== null);
    // Send and close wait for this, so they find the files in the fields.
    if (added.length > 0) deliver(draft, added);
    pending.delete(done);
    if (pending.size === 0 && uploads.get(draft) === pending) uploads.delete(draft);
    const keys = new Set(picked.map((p) => p.key));
    setAttaching(draft, (a) => a.filter((x) => !keys.has(x.key)));
    return added.length === results.length;
  };
  const done = store();
  pending.add(done);
}

/**
 * Files now in the draft join the composer that has it: on screen, or closed
 * and waiting. Send and close wait for every file, so none should be left
 * without one; if one is, the user hears that it is not in the draft.
 */
function deliver(draft: string, added: DraftAttachment[]) {
  // The user threw the draft away, files and all.
  if (discarded.has(draft)) return;
  const to = mountedComposer(draft) ?? waiting.get(draft)?.handle ?? null;
  if (to) to.addAttachments(added);
  else if (draftOnScreen() === draft) owed.set(draft, [...(owed.get(draft) ?? []), ...added]);
  else
    toast.error(
      `Couldn't attach ${added.map((a) => a.filename).join(", ")}: the draft closed before it was in`,
    );
}

/**
 * Null when no file is attaching to the draft; else resolves once none is,
 * files picked meanwhile included, to whether all of them were attached.
 */
function filesIn(draft: string): Promise<boolean> | null {
  const pending = uploads.get(draft);
  if (!pending || pending.size === 0) return null;
  const all = Promise.all(pending);
  return (async () => {
    const attached = (await all).every(Boolean);
    const later = filesIn(draft);
    return (later ? await later : true) && attached;
  })();
}

/**
 * Null when the composer can be saved whole now; else resolves once its
 * first load is in and its files are attached, to whether all of them were.
 */
function whenReady(draft: string): Promise<boolean> | null {
  const files = filesIn(draft);
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
  if (!files && !load) return null;
  return Promise.all([load, files]).then(([, ok]) => ok ?? true);
}

// ------------------------------------------------------------ opening and leaving

/** The draft in the composer on screen, inline or full-screen. */
export const draftOnScreen = (): string | null => composerOnScreen()?.draft ?? null;

/**
 * Subscribes a composer to its draft. A draft whose closed composer's close
 * or send still waits for its files is open again: that is cancelled, and
 * the closed composer's edits are saved first, so the new composer's first
 * load, on a new subscription, holds them (SPEC Drafts).
 */
function open(draft: string) {
  const left = cancelWaiting(draft);
  if (!left) {
    subscribe(composerKey(draft));
    return;
  }
  const save = left.handle?.pending() ?? null;
  if (save) act({ type: "saveDraft", draft, save });
  resubscribe(composerKey(draft));
}

/** Opens a composer that replaces the screen; closing it shows the screen beneath again. */
export function openScreen(draft: string) {
  open(draft);
  useCompose.setState({ onScreen: { draft, thread: null }, blurred: false });
}

/** Opens the reply composer under the open conversation, with its target message selected. */
export function openInline(draft: string, thread: number, target: number | null) {
  open(draft);
  useCompose.setState({ onScreen: { draft, thread }, blurred: false });
  // Whenever a reply composer opens, its target message is selected.
  if (target !== null) useChrome.getState().selectMessage(target);
}

/** Esc presses that blurred compose: a focus asked for before one never lands after it. */
let blurs = 0;

/** The first Esc: the editor loses focus and compose stays on screen. */
export function blurComposer() {
  blurs += 1;
  if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
  useCompose.setState({ blurred: true });
}

/** Moves focus in the next frame, once a field just shown is drawn, unless an Esc blurs compose first. */
export function focusNextFrame(focus: () => void) {
  const at = blurs;
  requestAnimationFrame(() => {
    if (blurs === at) focus();
  });
}

/** Focus came back into the composer. */
export function composerFocused() {
  if (composeBlurred()) useCompose.setState({ blurred: false });
}

/** A close or send that waits for its composer's first load or files. */
type Waiting = {
  /** The composer's handle, for its pending save; null if it never mounted. */
  handle: ComposerHandle | null;
  /** Resolves once the close or send is sent, given up, or cancelled. */
  done: Promise<void>;
  /** Reopening the draft cancelled it. */
  cancelled: boolean;
};

/** Closes and sends that wait, by draft. */
const waiting = new Map<string, Waiting>();

/** Cancels the close or send of `draft` that waits, if one does, and returns it. */
function cancelWaiting(draft: string): Waiting | undefined {
  const left = waiting.get(draft);
  if (!left) return undefined;
  left.cancelled = true;
  waiting.delete(draft);
  return left;
}

/** Runs `then` once `ready` resolves, with the draft waiting meanwhile, unless reopening or discarding cancels it. */
function defer(
  draft: string,
  handle: ComposerHandle | null,
  ready: Promise<boolean>,
  then: (ok: boolean) => void,
) {
  const entry: Waiting = { handle, done: Promise.resolve(), cancelled: false };
  entry.done = (async () => {
    const ok = await ready;
    if (entry.cancelled) return;
    waiting.delete(draft);
    then(ok);
  })();
  waiting.set(draft, entry);
}

/**
 * Closes the composer on screen as a second Esc would: kept as a draft, or
 * discarded if untouched.
 */
export function closeComposerOnScreen() {
  const draft = draftOnScreen();
  if (!draft) return;
  // This composer's own handle: the close may run after it has left the screen.
  const handle = mountedComposer(draft);
  // Typed text becomes a chip now, while its field is on screen: a close or
  // send that waits for a file runs after the fields are gone.
  handle?.commitTyped();
  // A send waiting for its files goes on once they are in (SPEC Drafts).
  if (waiting.has(draft)) {
    leave(draft, false);
    return;
  }
  const ready = whenReady(draft);
  // Off screen at once. The close itself waits for the first load and for
  // files still attaching, or it could discard the draft as untouched.
  leave(draft, false);
  if (ready) defer(draft, handle, ready, () => close(draft, handle));
  else close(draft, handle);
}

/**
 * Cmd+Q: closes the composer on screen, and resolves once every close or
 * send still waiting for a first load or a file is sent (SPEC UI state,
 * Actions).
 */
export function closeComposers(): Promise<void> {
  closeComposerOnScreen();
  return Promise.all([...waiting.values()].map((w) => w.done)).then(() => undefined);
}

/** Sends a composer's close, with its pending save ahead of it. */
function close(draft: string, handle: ComposerHandle | null) {
  const save = handle?.pending() ?? null;
  if (save) act({ type: "saveDraft", draft, save });
  act({ type: "closeComposer", draft });
  forgetSub(composerKey(draft));
}

/** Whether the reply composer on screen belongs to a conversation that is gone, deleted from Gmail. */
function replyGone(): boolean {
  const thread = composerOnScreen()?.thread ?? null;
  if (thread === null) return false;
  const screen = useChrome.getState().current()?.screen;
  if (screen?.kind !== "conversation" || screen.thread !== thread) return false;
  return conversationData(useMirror.getState(), thread, screen.trashMode)?.gone === true;
}

/**
 * A gone conversation draws no composer, so a reply composer in one closes
 * as a second Esc would, the draft kept (SPEC Reply composer). It closes as
 * the news arrives, before the composer leaves the screen, so its unsaved
 * text is saved. Call once at startup.
 */
export function closeGoneReplies() {
  useMirror.subscribe(closeIfGone);
  useCompose.subscribe(closeIfGone);
}

function closeIfGone() {
  if (replyGone()) closeComposerOnScreen();
}

/** Takes the composer off screen and, unless told not to, drops its subscription. */
function leave(draft: string, forget = true) {
  if (forget) forgetSub(composerKey(draft));
  if (draftOnScreen() === draft) useCompose.setState({ onScreen: null, blurred: false });
  if (mounted?.draft === draft) mounted = null;
}

// ------------------------------------------------------------ send and discard

/** Gets the draft back after Rust refused its send (compose/sends.ts, which this module can't import). */
type Refused = (account: number, draft: string, thread: number | null, why: string) => void;

/** Cmd+Enter: refuses with a toast while a recipient is invalid. */
export function sendOnScreen(refused: Refused) {
  const draft = draftOnScreen();
  if (!draft || waiting.has(draft)) return;
  const handle = mountedComposer(draft);
  const ready = whenReady(draft);
  if (!ready) {
    send(draft, handle, refused);
    return;
  }
  // Before the first load or while a file is still attaching, the draft is
  // not complete: it sends once it is, even if the composer is closed
  // meanwhile, and not at all if a file fails. The composer stays on screen.
  defer(draft, handle, ready, (ok) => {
    if (ok) {
      send(draft, handle, refused);
      return;
    }
    toast.error("Not sent: a file could not be attached");
    closeIfLeft(draft, handle);
  });
}

/** A send that did not go: a composer closed while it waited closes now, as a second Esc would. */
function closeIfLeft(draft: string, handle: ComposerHandle | null) {
  if (draftOnScreen() !== draft) close(draft, handle);
}

function send(draft: string, sentFrom: ComposerHandle | null, refused: Refused) {
  const handle = mountedComposer(draft) ?? sentFrom;
  // Taking the pending save ends it, so it is saved here, before any refusal.
  // It also turns typed text into chips, which the checks below must see.
  const save = handle?.pending() ?? null;
  if (save) act({ type: "saveDraft", draft, save });
  const invalid = handle?.invalid() ?? [];
  if (invalid.length > 0) {
    toast.error(
      `Fix ${invalid.length === 1 ? "this address" : "these addresses"}: ${invalid.join(", ")}`,
    );
    closeIfLeft(draft, handle);
    return;
  }
  const recipients = save?.fields ?? loadedDraft(draft);
  if (recipients && recipients.to.length + recipients.cc.length + recipients.bcc.length === 0) {
    toast.error("Add a recipient");
    closeIfLeft(draft, handle);
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

/** Cmd+Shift+,: discard locally and on Gmail, and with it a send that waits for a file. */
export function discardOnScreen() {
  const draft = draftOnScreen();
  if (!draft) return;
  discarded.add(draft);
  cancelWaiting(draft);
  act({ type: "discardDraft", draft });
  leave(draft);
  toast("Draft discarded");
}
