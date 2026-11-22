import { toast } from "sonner";
import { create } from "zustand";

import {
  act,
  addAttachment,
  errorText,
  failedWith,
  forgetSub,
  launch,
  resubscribe,
  subIdFor,
  subscribe,
} from "../ipc/bridge";
import type { Addr, DraftAttachment, DraftSave, DraftState, SubKey } from "../ipc/types";
import { dom } from "../keyboard/refs";
import { useChrome } from "../store/chrome";
import { conversationData } from "../store/current";
import { useMirror } from "../store/mirror";
import { composerDraft } from "../store/mirror-core";

export type UpdateSave = Extract<DraftSave, { type: "update" }>;

export type ComposeField = "to" | "cc" | "bcc" | "subject" | "body";

type ComposerHandle = {
  draft: string;
  pending: () => UpdateSave | null;
  commitTyped: () => void;
  invalid: () => string[];
  addAttachments: (added: DraftAttachment[]) => void;
  setFrom: (from: Addr) => void;
  attach: () => void;
  focus: (field: ComposeField) => void;
  editorFocused: () => boolean;
  openLink: () => void;
  closeLink: () => boolean;
};

export type Attaching = { key: number; name: string };

export type ComposeState = {
  onScreen: { draft: string; thread: number | null } | null;
  blurred: boolean;
  attaching: Record<string, Attaching[]>;
};

const useCompose = create<ComposeState>(() => ({ onScreen: null, blurred: false, attaching: {} }));

export const useComposeState = <T>(select: (s: ComposeState) => T): T => useCompose(select);

type ComposerKind = "inline" | "screen" | null;

export const composerKind = (s: ComposeState): ComposerKind =>
  s.onScreen === null ? null : s.onScreen.thread === null ? "screen" : "inline";

export const composeState = () => useCompose.getState();

export const composerOnScreen = () => useCompose.getState().onScreen;

export const composeBlurred = () => useCompose.getState().blurred;

let mounted: ComposerHandle | null = null;

const owed = new Map<string, DraftAttachment[]>();

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

export function mountedComposer(draft?: string): ComposerHandle | null {
  return mounted && (draft === undefined || mounted.draft === draft) ? mounted : null;
}

export const typingInComposer = () => mounted !== null && !composeBlurred();

export function inOverlay(): boolean {
  const { palette, sheet } = useChrome.getState();
  const search = dom.searchInput;
  return (
    palette !== null || sheet !== null || (search !== null && document.activeElement === search)
  );
}

const composerKey = (draft: string): SubKey => ({ type: "composer", draft });

export function loadedDraft(draft: string): DraftState | null {
  return composerDraft(useMirror.getState(), draft);
}

const uploads = new Map<string, Set<Promise<boolean>>>();
let attachSeq = 0;
const gone = new Set<string>();

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
          if (failedWith(err, "draftGone")) gone.add(draft);
          else toast.error(`Couldn't attach ${file.name}: ${errorText(err)}`);
          return null;
        }
      }),
    );
    const added = results.filter((a): a is DraftAttachment => a !== null);
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

function deliver(draft: string, added: DraftAttachment[]) {
  if (gone.has(draft)) return;
  const to = mountedComposer(draft) ?? waiting.get(draft)?.handle ?? null;
  if (to) to.addAttachments(added);
  else if (draftOnScreen() === draft) owed.set(draft, [...(owed.get(draft) ?? []), ...added]);
  else
    toast.error(
      `Couldn't attach ${added.map((a) => a.filename).join(", ")}: the draft closed before it was in`,
    );
}

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

const draftOnScreen = (): string | null => composerOnScreen()?.draft ?? null;

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

export function openScreen(draft: string) {
  open(draft);
  useCompose.setState({ onScreen: { draft, thread: null }, blurred: false });
}

export function openInline(draft: string, thread: number, target: number | null) {
  open(draft);
  useCompose.setState({ onScreen: { draft, thread }, blurred: false });
  if (target !== null) useChrome.getState().selectMessage(target);
}

let blurs = 0;

export function blurComposer() {
  blurs += 1;
  if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
  useCompose.setState({ blurred: true });
}

export function focusNextFrame(focus: () => void) {
  const at = blurs;
  requestAnimationFrame(() => {
    if (blurs === at) focus();
  });
}

export function composerFocused() {
  if (composeBlurred()) useCompose.setState({ blurred: false });
}

type Waiting = {
  handle: ComposerHandle | null;
  done: Promise<void>;
  cancelled: boolean;
};

const waiting = new Map<string, Waiting>();

function cancelWaiting(draft: string): Waiting | undefined {
  const left = waiting.get(draft);
  if (!left) return undefined;
  left.cancelled = true;
  waiting.delete(draft);
  return left;
}

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
    if (gone.has(draft)) return;
    then(ok);
  })();
  waiting.set(draft, entry);
}

export function closeComposerOnScreen() {
  const draft = draftOnScreen();
  if (!draft) return;
  const handle = mountedComposer(draft);
  handle?.commitTyped();
  if (waiting.has(draft)) {
    leave(draft, false);
    return;
  }
  const ready = whenReady(draft);
  leave(draft, false);
  if (ready) defer(draft, handle, ready, () => close(draft, handle));
  else close(draft, handle);
}

export function closeComposers(): Promise<void> {
  closeComposerOnScreen();
  return Promise.all([...waiting.values()].map((w) => w.done)).then(() => undefined);
}

function close(draft: string, handle: ComposerHandle | null) {
  const save = handle?.pending() ?? null;
  if (save) act({ type: "saveDraft", draft, save });
  act({ type: "closeComposer", draft });
  forgetSub(composerKey(draft));
}

function replyGone(): boolean {
  const thread = composerOnScreen()?.thread ?? null;
  if (thread === null) return false;
  const screen = useChrome.getState().current()?.screen;
  if (screen?.kind !== "conversation" || screen.thread !== thread) return false;
  return conversationData(useMirror.getState(), thread, screen.trashMode)?.gone === true;
}

export function closeGoneReplies() {
  useMirror.subscribe(closeIfGone);
  useCompose.subscribe(closeIfGone);
}

function closeIfGone() {
  if (replyGone()) closeComposerOnScreen();
}

export function leaveGoneDraft(draft: string) {
  if (draftOnScreen() !== draft) return;
  cancelWaiting(draft);
  leave(draft);
  toast("Draft was sent or deleted elsewhere");
}

function leave(draft: string, forget = true) {
  if (forget) forgetSub(composerKey(draft));
  if (draftOnScreen() === draft) useCompose.setState({ onScreen: null, blurred: false });
  if (mounted?.draft === draft) mounted = null;
}

type Refused = (account: number, draft: string, thread: number | null, why: string) => void;

export function sendOnScreen(refused: Refused) {
  const draft = draftOnScreen();
  if (!draft || waiting.has(draft)) return;
  const handle = mountedComposer(draft);
  const ready = whenReady(draft);
  if (!ready) {
    send(draft, handle, refused);
    return;
  }
  defer(draft, handle, ready, (ok) => {
    if (ok) {
      send(draft, handle, refused);
      return;
    }
    toast.error("Not sent: a file could not be attached");
    closeIfLeft(draft, handle);
  });
}

function closeIfLeft(draft: string, handle: ComposerHandle | null) {
  if (draftOnScreen() !== draft) close(draft, handle);
}

function send(draft: string, sentFrom: ComposerHandle | null, refused: Refused) {
  const handle = mountedComposer(draft) ?? sentFrom;
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

export function discardOnScreen() {
  const draft = draftOnScreen();
  if (!draft) return;
  gone.add(draft);
  cancelWaiting(draft);
  act({ type: "discardDraft", draft });
  leave(draft);
  toast("Draft discarded");
}
