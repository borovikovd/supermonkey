import { toast } from "sonner";

import { fixedHighlight, fixedTargets, stillShown } from "../acting";
import { openConversation, switchAccount } from "../navigation";
import { act } from "../ipc/bridge";
import type { ConversationData, DraftKind } from "../ipc/types";
import { useChrome } from "../store/chrome";
import { conversationData, openConversationData } from "../store/current";
import { useMirror } from "../store/mirror";
import { appState } from "../store/mirror-core";
import {
  closeComposerOnScreen,
  composerOnScreen,
  mountedComposer,
  openInline,
  openScreen,
} from "./session";

type ReplyMode = "reply" | "replyAll" | "forward";

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

function resumeDraft(draft: string, thread: number | null, target: number | null) {
  closeComposerOnScreen();
  if (thread === null) {
    openScreen(draft);
    return;
  }
  const cur = useChrome.getState().current();
  if (cur?.screen.kind !== "conversation" || cur.screen.thread !== thread) {
    openConversation(thread);
  }
  const screen = useChrome.getState().current()?.screen;
  const trashMode = screen?.kind === "conversation" && screen.trashMode;
  typedAhead ??= "";
  whenLoaded(thread, trashMode, (data) =>
    openInline(draft, thread, target ?? data.drafts.find((d) => d.id === draft)?.target ?? null),
  );
}

let typedAhead: string | null = null;

export function typeAhead(e: KeyboardEvent): boolean {
  if (typedAhead === null || e.metaKey || e.ctrlKey || e.altKey) return false;
  const screen = useChrome.getState().current()?.screen;
  const onScreen = composerOnScreen();
  if (screen?.kind !== "conversation" || (onScreen && mountedComposer(onScreen.draft))) {
    typedAhead = null;
    return false;
  }
  if (e.key.length === 1) typedAhead += e.key;
  else if (e.key === "Backspace") typedAhead = typedAhead.slice(0, -1);
  else return false;
  e.preventDefault();
  return true;
}

export function takeTypedAhead(): string {
  const text = typedAhead ?? "";
  typedAhead = null;
  return text;
}

let stopWaiting: (() => void) | null = null;

function whenLoaded(thread: number, trashMode: boolean, run: (data: ConversationData) => void) {
  stopWaiting?.();
  const ready = () => conversationData(useMirror.getState(), thread, trashMode);
  const aim = (data: ConversationData) => {
    if (!data.gone) run(data);
    else typedAhead = null;
  };
  const now = ready();
  if (now) {
    aim(now);
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
  const unsubChrome = useChrome.subscribe((s) => {
    if (onScreen() && s.palette === null && s.sheet === null) return;
    stop();
    typedAhead = null;
  });
  const unsubMirror = useMirror.subscribe(() => {
    cancelAnimationFrame(frame);
    frame = requestAnimationFrame(() => {
      const d = ready();
      if (!d) return;
      stop();
      aim(d);
    });
  });
  stopWaiting = stop;
}

export function startReply(mode: ReplyMode) {
  const c = useChrome.getState();
  const { account } = c;
  const cur = c.current();
  if (account === null || !cur) return;

  let fromList = false;
  if (cur.screen.kind === "list") {
    const row =
      fixedTargets() === null
        ? cur.highlight
        : (stillShown([Number(fixedHighlight())]).map(String)[0] ?? null);
    if (row === null) return;
    const thread = Number(row);
    if (Number.isNaN(thread)) return;
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

export function resumeDraftById(draft: string) {
  const data = openConversationData();
  const own = data?.drafts.find((d) => d.id === draft);
  if (own && data) {
    resumeDraft(draft, data.thread, own.target);
    return;
  }
  resumeDraft(draft, useMirror.getState().rows[draft]?.thread ?? null, null);
}

export function reopenDraft(account: number, draft: string, thread: number | null) {
  if (!appState(useMirror.getState())?.accounts.some((a) => a.id === account)) {
    toast("That account was removed");
    return;
  }
  switchAccount(account);
  const c = useChrome.getState();
  c.closePalette();
  c.closeSheet();
  resumeDraft(draft, thread, null);
}
