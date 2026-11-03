// Starting, resuming, sending, and closing composers (SPEC Compose, Reply
// composer, Keyboard). The open composer registers its pending save here, so
// send, discard, and close carry it ahead of them in the same invoke.

import { toast } from "sonner";

import { markReadOnOpenWhenLoaded, openThread } from "../commands";
import { act, forgetSub, subIdFor, subscribe } from "../ipc/bridge";
import type {
  Addr,
  ConversationData,
  DraftFields,
  DraftKind,
  DraftState,
  SubKey,
} from "../ipc/types";
import { conversationRef } from "../keyboard/refs";
import { useChrome, type Screen } from "../store/chrome";
import { useMirror } from "../store/mirror";

export type ReplyMode = "reply" | "replyAll" | "forward" | "resumeDraft";

/** The open composer's hooks, set while it is mounted. */
export const openComposer: {
  draft: string | null;
  /** Pending fields not yet saved, or null when saved. */
  pending: () => DraftFields | null;
  /** Recipients that are not valid addresses. */
  invalid: () => string[];
  focus: () => void;
  setFrom: (from: Addr) => void;
  attach: () => void;
  /** Files still being attached: resolves to whether all of them were; null when none are. */
  uploads: () => Promise<boolean> | null;
} = {
  draft: null,
  pending: () => null,
  invalid: () => [],
  focus: () => {},
  setFrom: () => {},
  attach: () => {},
  uploads: () => null,
};

const composerKey = (draft: string): SubKey => ({ type: "composer", draft });

/** The composer's draft as its subscription last brought it; null before its first load. */
export function loadedDraft(draft: string): DraftState | null {
  for (const sub of Object.values(useMirror.getState().subs)) {
    if (sub.type === "composer" && sub.key.draft === draft) return sub.draft;
  }
  return null;
}

/**
 * Null when the composer on screen can be saved whole now; else resolves
 * once its first load is in and its files are attached, to whether all of
 * them were.
 */
function whenReady(draft: string): Promise<boolean> | null {
  const uploads = openComposer.draft === draft ? openComposer.uploads() : null;
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

/** Screens a `c` composer returns to, per draft. */
const returnTo = new Map<string, Screen>();

/** The draft in the composer on screen, inline or full-screen. */
function draftOnScreen(): string | null {
  const c = useChrome.getState();
  const cur = c.current();
  return c.inline?.draft ?? (cur?.screen.kind === "compose" ? cur.screen.draft : null);
}

/** Opens a composer that replaces the screen. */
function openScreen(draft: string) {
  subscribe(composerKey(draft));
  const c = useChrome.getState();
  returnTo.set(draft, c.current()?.screen ?? { kind: "list" });
  c.patchCurrent({ screen: { kind: "compose", draft } });
  c.set({ composeBlurred: false, inline: null });
}

/** Opens the reply composer under the open conversation. */
function openInline(draft: string, thread: number, target: number | null) {
  subscribe(composerKey(draft));
  const c = useChrome.getState();
  c.set({
    inline: { draft, thread },
    composeBlurred: false,
    // Whenever a reply composer opens, its target message is selected.
    selectedMessage: target ?? c.selectedMessage,
  });
}

/** `c`: a new message that replaces the screen. */
export function startCompose() {
  const c = useChrome.getState();
  if (c.account === null) return;
  closeComposerOnScreen();
  const draft = crypto.randomUUID();
  act({
    type: "saveDraft",
    draft,
    save: {
      type: "create",
      account: c.account,
      kind: "new",
      replyAll: false,
      thread: null,
      target: null,
      startedOnLatest: false,
    },
  });
  openScreen(draft);
}

/** Opens a draft from the Drafts list or a collapsed draft in a thread. */
export function resumeDraft(draft: string, thread: number | null, target: number | null) {
  closeComposerOnScreen();
  if (thread === null) {
    openScreen(draft);
    return;
  }
  const cur = useChrome.getState().current();
  if (cur?.screen.kind !== "conversation" || cur.screen.thread !== thread) {
    openThread(thread);
    markReadOnOpenWhenLoaded(thread);
  }
  openInline(draft, thread, target);
}

/** Waits for the conversation's data (first load) before aiming a composer. */
function whenLoaded(thread: number, run: (data: ConversationData) => void) {
  const ready = () => {
    const d = conversationRef.data;
    return d && d.thread === thread ? d : null;
  };
  const now = ready();
  if (now) {
    run(now);
    return;
  }
  // Several mirror updates can queue frames before one finds the data; only
  // the first may run, or each would start its own draft.
  let done = false;
  const unsub = useMirror.subscribe(() => {
    // conversationRef updates in an effect after the mirror changes.
    requestAnimationFrame(() => {
      const d = ready();
      if (d && !done) {
        done = true;
        unsub();
        run(d);
      }
    });
  });
  setTimeout(unsub, 5000);
}

/**
 * `r`, `Enter`, `f`: resume the newest draft of that kind aimed at the
 * selected message; with the selection on the latest message, the newest one
 * started on the latest; else start a new one aimed at the selected message.
 */
export function startReply(mode: ReplyMode, draftId?: string) {
  const c = useChrome.getState();
  if (c.account === null) return;
  const cur = c.current();
  if (!cur) return;

  if (mode === "resumeDraft" && draftId) {
    const row = useMirror.getState().rows[draftId];
    resumeDraftById(draftId, row?.thread ?? null);
    return;
  }

  let thread: number;
  let fromList = false;
  if (cur.screen.kind === "conversation") {
    thread = cur.screen.thread;
  } else if (cur.screen.kind === "list" && cur.highlight !== null) {
    if (cur.where.kind === "view" && cur.where.view.kind === "drafts") {
      resumeDraftById(cur.highlight, useMirror.getState().rows[cur.highlight]?.thread ?? null);
      return;
    }
    thread = Number(cur.highlight);
    if (Number.isNaN(thread)) return;
    // `r` and `f` on the list open the conversation first and aim at its latest message.
    openThread(thread);
    markReadOnOpenWhenLoaded(thread);
    fromList = true;
  } else return;

  closeComposerOnScreen();
  const account = c.account;
  whenLoaded(thread, (data) => {
    const msgs = data.messages;
    const latest = msgs.at(-1)?.id ?? null;
    const selected = fromList ? latest : (useChrome.getState().selectedMessage ?? latest);
    const kind: DraftKind = mode === "forward" ? "forward" : "reply";
    const ofKind = data.drafts
      .filter((d) => d.kind === kind)
      .toSorted((a, b) => b.updatedAt - a.updatedAt);
    const resume =
      ofKind.find((d) => d.target === selected) ??
      (selected === latest ? ofKind.find((d) => d.startedOnLatest) : undefined);
    if (resume) {
      openInline(resume.id, thread, resume.target);
      return;
    }
    const draft = crypto.randomUUID();
    act({
      type: "saveDraft",
      draft,
      save: {
        type: "create",
        account,
        kind,
        replyAll: mode === "replyAll",
        thread,
        target: selected,
        startedOnLatest: selected === latest,
      },
    });
    openInline(draft, thread, selected);
  });
}

function resumeDraftById(draft: string, thread: number | null) {
  const data = conversationRef.data;
  const own = data?.drafts.find((d) => d.id === draft);
  resumeDraft(draft, own && data ? data.thread : thread, own?.target ?? null);
}

/** Closes the composer on screen as a second Esc would: kept as a draft, or discarded if untouched. */
export function closeComposerOnScreen() {
  const draft = draftOnScreen();
  if (!draft) return;
  // This composer's own hook: the close may run after it has left the screen.
  const pending = openComposer.draft === draft ? openComposer.pending : () => null;
  const wait = whenReady(draft);
  const close = () => {
    const fields = pending();
    if (fields) act({ type: "saveDraft", draft, save: { type: "update", fields } });
    act({ type: "closeComposer", draft });
    forgetSub(composerKey(draft));
  };
  // Off screen at once. The close itself waits for the first load and for
  // files still attaching, or it could discard the draft as untouched.
  leave(draft, false);
  if (wait) void wait.then(close);
  else close();
}

/** Takes the composer off screen and, unless told not to, drops its subscription. */
function leave(draft: string, forget = true) {
  if (forget) forgetSub(composerKey(draft));
  const c = useChrome.getState();
  if (c.inline?.draft === draft) c.set({ inline: null, composeBlurred: false });
  const cur = c.current();
  if (cur?.screen.kind === "compose" && cur.screen.draft === draft) {
    c.patchCurrent({ screen: returnTo.get(draft) ?? { kind: "list" } });
    returnTo.delete(draft);
    c.set({ composeBlurred: false });
  }
  if (openComposer.draft === draft) openComposer.draft = null;
}

/** Cmd+Enter: refuses with a toast while a recipient is invalid. */
export function sendOnScreen() {
  const draft = draftOnScreen();
  if (!draft) return;
  // Before the first load or while a file is still attaching, the draft is
  // not complete: send once it is, and not at all if a file fails. The
  // composer stays on screen meanwhile.
  const wait = whenReady(draft);
  if (wait) {
    void wait.then((ok) => {
      if (draftOnScreen() !== draft) return;
      if (ok) sendOnScreen();
      else toast.error("Not sent: a file could not be attached");
    });
    return;
  }
  // Taking the pending save ends it, so it is saved here, before any refusal.
  // It also turns typed text into chips, which the checks below must see.
  const fields = openComposer.draft === draft ? openComposer.pending() : null;
  if (fields) act({ type: "saveDraft", draft, save: { type: "update", fields } });
  const invalid = openComposer.draft === draft ? openComposer.invalid() : [];
  if (invalid.length > 0) {
    toast.error(
      `Fix ${invalid.length === 1 ? "this address" : "these addresses"}: ${invalid.join(", ")}`,
    );
    return;
  }
  const draftState = Object.values(useMirror.getState().subs).find(
    (s) => s.type === "composer" && s.key.type === "composer" && s.key.draft === draft,
  );
  const recipients =
    fields ?? (draftState?.type === "composer" && draftState.draft ? draftState.draft : null);
  if (recipients && recipients.to.length + recipients.cc.length + recipients.bcc.length === 0) {
    toast.error("Add a recipient");
    return;
  }
  act({ type: "sendDraft", draft });
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

/**
 * Puts a draft back in a composer after undo, a held undo that resolved as
 * not sent, or a rejection. Switches account and reopens its thread.
 */
export function reopenDraft(account: number, draft: string, thread: number | null) {
  const c = useChrome.getState();
  if (c.account !== account) {
    closeComposerOnScreen();
    c.setAccount(account);
  }
  // Reopens the thread even if it has left the current view.
  resumeDraft(draft, thread, null);
}
