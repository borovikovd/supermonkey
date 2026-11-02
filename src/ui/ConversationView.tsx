import { File, Paperclip } from "lucide-react";
import { useEffect, useLayoutEffect, useRef } from "react";

import { markReadOnOpen } from "../commands";
import { Composer } from "../compose/Composer";
import { startReply } from "../compose/start";
import { openAttachment } from "../ipc/bridge";
import type { AttachmentInfo, DraftSummary, MessageSummary, OutboxRow } from "../ipc/types";
import { onKeyDown } from "../keyboard/keys";
import { appHooks, conversationRef } from "../keyboard/refs";
import { useChrome } from "../store/chrome";
import { useAppState, useConversation, useSubscription } from "../store/hooks";
import { formatTime } from "./format";
import { MessageFrame } from "./MessageFrame";

const name = (m: MessageSummary) => m.from.name || m.from.email;

function fullDate(ms: number) {
  return new Date(ms).toLocaleString(undefined, {
    weekday: "short",
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  });
}

function recipients(m: MessageSummary) {
  const all = [...m.to, ...m.cc].map((a) => a.name || a.email);
  return all.length > 0 ? `to ${all.join(", ")}` : "";
}

/** Clicking a message selects it (SPEC Reading). Stable, so frames keep their listeners. */
const select = (id: number) => useChrome.getState().set({ selectedMessage: id });

/** Expands or collapses a message, and selects it. */
const toggle = (id: number) => {
  const c = useChrome.getState();
  const ex = new Set(c.expanded);
  if (ex.has(id)) ex.delete(id);
  else ex.add(id);
  c.set({ expanded: [...ex], selectedMessage: id });
};

type Block = { type: "message"; m: MessageSummary } | { type: "fold"; ids: number[] };

/**
 * Collapsed runs of more than four fold the ones between the first and last
 * into one `N older messages` line (SPEC Reading).
 */
function blocks(messages: MessageSummary[], expanded: Set<number>, unfolded: Set<number>): Block[] {
  const out: Block[] = [];
  let run: MessageSummary[] = [];
  const flush = () => {
    const head = run[0];
    const tail = run.at(-1);
    if (run.length > 4 && head && tail && !run.some((m) => unfolded.has(m.id))) {
      out.push({ type: "message", m: head });
      out.push({ type: "fold", ids: run.slice(1, -1).map((m) => m.id) });
      out.push({ type: "message", m: tail });
    } else {
      for (const m of run) out.push({ type: "message", m });
    }
    run = [];
  };
  for (const m of messages) {
    if (expanded.has(m.id)) {
      flush();
      out.push({ type: "message", m });
    } else run.push(m);
  }
  flush();
  return out;
}

/** Tab cycles links in expanded messages; Enter opens; Esc clears. */
function linkKey(e: KeyboardEvent): boolean {
  const c = useChrome.getState();
  if (e.key === "Tab") {
    const frames = Array.from(document.querySelectorAll<HTMLIFrameElement>("iframe[data-frame]"));
    const links: [number, HTMLAnchorElement][] = [];
    for (const f of frames) {
      const id = Number(f.dataset.frame);
      for (const a of Array.from(
        f.contentDocument?.querySelectorAll<HTMLAnchorElement>("a[href]") ?? [],
      )) {
        links.push([id, a]);
      }
    }
    for (const [, a] of links) a.classList.remove("ub-link-focus");
    if (links.length === 0) return true;
    const current = c.link;
    let at = current
      ? links.findIndex(
          ([id], i) => id === current[0] && i - links.findIndex(([x]) => x === id) === current[1],
        )
      : -1;
    at = (at + (e.shiftKey ? -1 : 1) + links.length) % links.length;
    const picked = links[at];
    if (!picked) return true;
    const [id, a] = picked;
    const first = links.findIndex(([x]) => x === id);
    a.classList.add("ub-link-focus");
    a.scrollIntoView({ block: "nearest" });
    a.ownerDocument.defaultView?.frameElement?.scrollIntoView({
      block: "nearest",
    });
    c.set({ link: [id, at - first] });
    e.preventDefault();
    return true;
  }
  if (c.link && e.key === "Enter") {
    const f = document.querySelector<HTMLIFrameElement>(`iframe[data-frame="${c.link[0]}"]`);
    const a = f?.contentDocument?.querySelectorAll<HTMLAnchorElement>("a[href]")[c.link[1]];
    a?.click();
    e.preventDefault();
    return true;
  }
  if (c.link && e.key === "Escape") {
    for (const f of Array.from(
      document.querySelectorAll<HTMLIFrameElement>("iframe[data-frame]"),
    )) {
      f.contentDocument
        ?.querySelectorAll(".ub-link-focus")
        .forEach((x) => x.classList.remove("ub-link-focus"));
    }
    c.set({ link: null });
    e.preventDefault();
    return true;
  }
  return false;
}

export function ConversationView({ thread, trashMode }: { thread: number; trashMode: boolean }) {
  useSubscription({ type: "conversation", thread, trashMode, open: true });
  const data = useConversation(thread, trashMode);
  const expandedArr = useChrome((s) => s.expanded);
  const unfoldedArr = useChrome((s) => s.unfolded);
  const expandAll = useChrome((s) => s.expandAll);
  const selected = useChrome((s) => s.selectedMessage);
  const inline = useChrome((s) => s.inline);
  const remoteImages = useAppState()?.settings.remoteImages ?? true;
  const known = useRef<Set<number> | null>(null);
  const latestRef = useRef<number | null>(null);

  useEffect(() => {
    conversationRef.data = data;
  }, [data]);
  useEffect(() => {
    appHooks.linkKey = linkKey;
    return () => {
      conversationRef.data = null;
    };
  }, []);

  // Fix expansion and scroll from the unread state when the conversation opens;
  // later arrivals expand when they are the new latest or unread.
  useLayoutEffect(() => {
    if (!data || data.gone) return;
    const c = useChrome.getState();
    const msgs = data.messages;
    const latest = msgs.at(-1)?.id ?? null;
    if (known.current === null) {
      known.current = new Set(msgs.map((m) => m.id));
      const expanded = msgs.filter((m) => m.unread).map((m) => m.id);
      if (latest !== null) expanded.push(latest);
      c.set({ expanded, selectedMessage: latest });
      latestRef.current = latest;
      const firstUnread = msgs.find((m) => m.unread)?.id ?? latest;
      requestAnimationFrame(() => {
        if (firstUnread !== null)
          document
            .querySelector(`[data-message="${firstUnread}"]`)
            ?.scrollIntoView({ block: "start" });
      });
      return;
    }
    const seen = known.current;
    const added = msgs.filter((m) => !seen.has(m.id));
    for (const m of added) seen.add(m.id);
    let expanded = c.expanded;
    let sel = c.selectedMessage;
    const fresh = added.filter((m) => m.id === latest || m.unread).map((m) => m.id);
    if (fresh.length > 0) expanded = [...expanded, ...fresh];
    // Selection follows a new latest message when it sat on the previous latest.
    if (latest !== latestRef.current && sel === latestRef.current) sel = latest;
    if (sel !== null && !msgs.some((m) => m.id === sel)) sel = latest;
    latestRef.current = latest;
    if (expanded !== c.expanded || sel !== c.selectedMessage)
      c.set({ expanded, selectedMessage: sel });
    // A message arriving unread while open in the focused window is marked read.
    if (added.some((m) => m.unread) && document.hasFocus()) markReadOnOpen(thread);
  }, [data, thread]);

  if (!data) return <div className="flex-1" />;
  if (data.gone)
    return (
      <div className="flex flex-1 items-center justify-center text-text-2">
        This conversation is gone.
      </div>
    );

  const expanded = new Set(expandAll ? data.messages.map((m) => m.id) : expandedArr);
  // The selected message never hides in a fold.
  const unfolded = new Set(selected === null ? unfoldedArr : [...unfoldedArr, selected]);

  return (
    <div className="min-h-0 flex-1 overflow-y-auto">
      <div className="mx-auto w-full max-w-[768px] min-w-[520px] px-16 pt-[11px] pb-24">
        <h1 className="selectable mb-6 text-subject font-semibold">
          {data.subject || "(no subject)"}
        </h1>
        {blocks(data.messages, expanded, unfolded).map((b) =>
          b.type === "fold" ? (
            <Fold key={`fold-${b.ids[0]}`} ids={b.ids} />
          ) : (
            <Message
              key={b.m.id}
              m={b.m}
              expanded={expanded.has(b.m.id)}
              selected={selected === b.m.id}
              remoteImages={remoteImages}
              quoteOpen={expandAll}
            />
          ),
        )}
        {data.outbox
          .filter((o) => o.state !== "sent")
          .map((o) => (
            <PendingSend key={`ob${o.id}`} row={o} />
          ))}
        {data.drafts
          .filter((d) => d.id !== inline?.draft)
          .map((d) => (
            <DraftStub key={d.id} draft={d} />
          ))}
        {inline?.thread === thread && <Composer key={inline.draft} draft={inline.draft} inline />}
      </div>
    </div>
  );
}

/** A folded run of collapsed messages; a click unfolds it. */
function Fold({ ids }: { ids: number[] }) {
  return (
    <button
      onClick={() => {
        const c = useChrome.getState();
        c.set({ unfolded: [...c.unfolded, ...ids] });
      }}
      // Reads as one more collapsed row: same height, same divider.
      className="flex h-row w-full items-center gap-3 border-b border-border-2 text-left text-row text-text-2 hover:bg-fill-half"
    >
      <span className="flex h-5 min-w-5 items-center justify-center rounded-full bg-fill px-1.5 text-hint font-medium text-text-2">
        {ids.length}
      </span>
      older messages
    </button>
  );
}

/** A send still in the outbox, shown at the end of its thread. */
function PendingSend({ row }: { row: OutboxRow }) {
  return (
    <div className="my-2 flex items-center gap-3 rounded-control bg-surface px-4 py-2 text-row">
      <span className="font-medium">Me</span>
      <span className="truncate text-text-2">{row.snippet}</span>
      <span className="ml-auto text-hint text-accent">
        {row.delayed ? "Email delayed" : "Sending"}
      </span>
    </div>
  );
}

/** A closed reply or forward draft; a click reopens it. */
function DraftStub({ draft }: { draft: DraftSummary }) {
  return (
    <button
      onClick={() => startReply("resumeDraft", draft.id)}
      className="my-1 flex w-full items-center gap-3 rounded-control bg-surface px-4 py-2 text-left text-row hover:bg-fill-half"
    >
      <File size={14} strokeWidth={1.5} className="text-danger" />
      <span className="font-medium text-danger">Draft</span>
      <span className="truncate text-text-2">
        {draft.snippet || (draft.kind === "forward" ? "Forward" : "Reply")}
      </span>
    </button>
  );
}

function Message({
  m,
  expanded,
  selected,
  remoteImages,
  quoteOpen,
}: {
  m: MessageSummary;
  expanded: boolean;
  selected: boolean;
  remoteImages: boolean;
  quoteOpen: boolean;
}) {
  const bar = selected ? (
    <span className="absolute top-2 bottom-2 -left-5 w-[2px] rounded-full bg-accent" />
  ) : null;
  if (!expanded) {
    return (
      <button
        type="button"
        data-message={m.id}
        aria-expanded={false}
        onClick={() => toggle(m.id)}
        className="relative flex h-row w-full items-center gap-3 border-b border-border-2 text-left text-row hover:bg-fill-half"
      >
        {bar}
        <span
          className={`w-[160px] shrink-0 truncate ${m.unread ? "font-semibold" : "font-medium text-text-2"}`}
        >
          {name(m)}
        </span>
        <span className="flex-1 truncate text-text-2">{m.snippet}</span>
        <span className="tabular shrink-0 text-time text-text-3">{formatTime(m.date)}</span>
      </button>
    );
  }
  const files = m.attachments.filter((a) => !a.inline);
  return (
    // Clicking selects the message, a pointer shortcut for `n` / `p`.
    // oxlint-disable-next-line jsx-a11y/no-static-element-interactions
    <div
      data-message={m.id}
      className="relative border-b border-border-2 py-4"
      onMouseDown={() => select(m.id)}
    >
      {bar}
      <button
        type="button"
        aria-expanded
        className="mb-5 flex w-full items-baseline gap-2 text-left text-row"
        onClick={() => toggle(m.id)}
      >
        <span className="font-semibold">{name(m)}</span>
        <span className="truncate text-text-3">{recipients(m)}</span>
        <span className="tabular ml-auto shrink-0 text-time text-text-3">{fullDate(m.date)}</span>
      </button>
      <div className="selectable">
        <MessageFrame
          id={m.id}
          hasBody={m.hasBody}
          onSelect={select}
          remoteImages={remoteImages}
          quoteOpen={quoteOpen}
          onKey={onKeyDown}
        />
      </div>
      {files.length > 0 && <Attachments files={files} />}
    </div>
  );
}

function Attachments({ files }: { files: AttachmentInfo[] }) {
  return (
    <div className="mt-3 flex flex-wrap gap-2">
      {files.map((a) => (
        <button
          key={a.id}
          onClick={() => void openAttachment(a.id)}
          className="flex items-center gap-2 rounded-control border border-border px-2 py-1 text-row hover:bg-fill-half"
        >
          <Paperclip size={12} strokeWidth={1.5} className="text-text-3" />
          <span className="max-w-[200px] truncate">{a.filename}</span>
          <span className="text-hint text-text-3">{Math.max(1, Math.round(a.size / 1024))} KB</span>
        </button>
      ))}
    </div>
  );
}
