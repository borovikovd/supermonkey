// Starting and resuming composers (SPEC Compose, Reply composer, Keyboard):
// `c`, `r`, `Enter`, `f`, a draft from Drafts or a thread, and a draft that
// comes back after an undone or refused send.

import { openConversation, switchAccount } from "../navigation";
import { act } from "../ipc/bridge";
import type { ConversationData, DraftKind } from "../ipc/types";
import { useChrome } from "../store/chrome";
import { conversationData, openConversationData } from "../store/current";
import { useMirror } from "../store/mirror";
import {
  closeComposerOnScreen,
  composerOnScreen,
  mountedComposer,
  openInline,
  openScreen,
} from "./session";

export type ReplyMode = "reply" | "replyAll" | "forward" | "resumeDraft";

/** `c`: a new message that replaces the screen. */
export function startCompose() {
  const { account } = useChrome.getState();
  if (account === null) return;
  closeComposerOnScreen();
  const draft = crypto.randomUUID();
  act({
    type: "saveDraft",
    draft,
    save: {
      type: "create",
      account,
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
    openConversation(thread);
  }
  openInline(draft, thread, target);
}

/**
 * Text typed after a reply key and before its editor takes it, or null when
 * no reply is on its way. Those keys are the start of the reply, not
 * shortcuts.
 */
let typedAhead: string | null = null;

/** Keydown: keeps a key typed while a reply is on its way; true if kept. */
export function typeAhead(e: KeyboardEvent): boolean {
  if (typedAhead === null || e.metaKey || e.ctrlKey || e.altKey) return false;
  const screen = useChrome.getState().current()?.screen;
  const onScreen = composerOnScreen();
  if (screen?.kind !== "conversation" || (onScreen && mountedComposer(onScreen.draft))) {
    // The reply was dropped, or its editor is up and took the text.
    typedAhead = null;
    return false;
  }
  if (e.key.length === 1) typedAhead += e.key;
  else if (e.key === "Backspace") typedAhead = typedAhead.slice(0, -1);
  else return false;
  e.preventDefault();
  return true;
}

/** The reply editor takes what was typed ahead of it. */
export function takeTypedAhead(): string {
  const text = typedAhead ?? "";
  typedAhead = null;
  return text;
}

/** Stops the reply waiting for its conversation's data, if one is. */
let stopWaiting: (() => void) | null = null;

/**
 * Waits for the conversation's data (first load) before aiming a composer.
 * Only the latest reply key waits, or each would start its own draft, and it
 * waits only while that conversation stays on screen.
 */
function whenLoaded(thread: number, trashMode: boolean, run: (data: ConversationData) => void) {
  stopWaiting?.();
  const ready = () => conversationData(useMirror.getState(), thread, trashMode);
  const now = ready();
  if (now) {
    run(now);
    return;
  }
  const onScreen = () => {
    const screen = useChrome.getState().current()?.screen;
    return screen?.kind === "conversation" && screen.thread === thread;
  };
  let frame = 0;
  const stop = () => {
    unsubMirror();
    unsubChrome();
    cancelAnimationFrame(frame);
    if (stopWaiting === stop) stopWaiting = null;
  };
  const unsubChrome = useChrome.subscribe(() => {
    if (onScreen()) return;
    stop();
    typedAhead = null;
  });
  const unsubMirror = useMirror.subscribe(() => {
    // Aim once the conversation has rendered its first load.
    cancelAnimationFrame(frame);
    frame = requestAnimationFrame(() => {
      const d = ready();
      if (!d) return;
      stop();
      run(d);
    });
  });
  stopWaiting = stop;
}

/**
 * `r`, `Enter`, `f`: resume the newest draft of that kind aimed at the
 * selected message; with the selection on the latest message, the newest one
 * started on the latest; else start a new one aimed at the selected message.
 */
export function startReply(mode: ReplyMode, draftId?: string) {
  const c = useChrome.getState();
  const { account } = c;
  const cur = c.current();
  if (account === null || !cur) return;

  if (mode === "resumeDraft" && draftId) {
    resumeDraftById(draftId);
    return;
  }

  let fromList = false;
  if (cur.screen.kind === "list") {
    if (cur.highlight === null) return;
    if (cur.where.kind === "view" && cur.where.view.kind === "drafts") {
      resumeDraftById(cur.highlight);
      return;
    }
    const thread = Number(cur.highlight);
    if (Number.isNaN(thread)) return;
    // `r` and `f` on the list open the conversation first and aim at its latest message.
    openConversation(thread);
    fromList = true;
  }
  const screen = useChrome.getState().current()?.screen;
  if (screen?.kind !== "conversation") return;
  const { thread, trashMode } = screen;

  closeComposerOnScreen();
  typedAhead ??= "";
  whenLoaded(thread, trashMode, (data) => {
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

/** A draft by id: in the open conversation, aimed at its target; else on its thread from the list row. */
function resumeDraftById(draft: string) {
  const data = openConversationData();
  const own = data?.drafts.find((d) => d.id === draft);
  if (own && data) {
    resumeDraft(draft, data.thread, own.target);
    return;
  }
  resumeDraft(draft, useMirror.getState().rows[draft]?.thread ?? null, null);
}

/**
 * Puts a draft back in a composer after undo, a held undo that resolved as
 * not sent, or a rejection. Switches account and reopens its thread, even if
 * it has left the current view.
 */
export function reopenDraft(account: number, draft: string, thread: number | null) {
  switchAccount(account);
  resumeDraft(draft, thread, null);
}
