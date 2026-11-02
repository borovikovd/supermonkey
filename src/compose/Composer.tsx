// The composer, inline under a conversation or replacing the screen (`c`).
// TipTap edits the body; the quote is a read-only block Rust builds.

import { HardBreak } from "@tiptap/extension-hard-break";
import { Strike } from "@tiptap/extension-strike";
import { Placeholder } from "@tiptap/extensions";
import { EditorContent, useEditor } from "@tiptap/react";
import { StarterKit } from "@tiptap/starter-kit";
import { ChevronDown, Paperclip, Trash2, X } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { toast } from "sonner";

import { act, addAttachment } from "../ipc/bridge";
import type { Addr, DraftAttachment, DraftFields } from "../ipc/types";
import { useChrome } from "../store/chrome";
import { useAppState, useDraft } from "../store/hooks";
import { RecipientField, formatAddr, isValidEmail, type RecipientHandle } from "./RecipientField";
import { closeComposerOnScreen, discardOnScreen, openComposer, sendOnScreen } from "./start";

const SAVE_DELAY = 500;

// Re-added with keys that do not clash with the compose table.
const StrikeKeys = Strike.extend({
  addKeyboardShortcuts() {
    return { "Mod-Shift-x": () => this.editor.commands.toggleStrike() };
  },
});
const HardBreakKeys = HardBreak.extend({
  addKeyboardShortcuts() {
    return { "Shift-Enter": () => this.editor.commands.setHardBreak() };
  },
});

function QuoteBlock({ html }: { html: string }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="mt-1">
      <button
        tabIndex={-1}
        onClick={() => setOpen(!open)}
        className="text-body leading-none tracking-widest text-text-3 hover:text-text-2"
        title="Show quoted text"
      >
        •••
      </button>
      {open && (
        <iframe
          title="quoted"
          sandbox="allow-same-origin"
          className="mt-2 w-full border-0"
          srcDoc={`<!doctype html><html><head><meta http-equiv="Content-Security-Policy" content="script-src 'none'"><style>body{margin:0;font:13px/20px -apple-system,sans-serif;color:#73716D}</style></head><body>${html}</body></html>`}
          onLoad={(e) => {
            const el = e.currentTarget;
            const doc = el.contentDocument;
            if (doc) el.style.height = `${doc.documentElement.scrollHeight}px`;
          }}
        />
      )}
    </div>
  );
}

/** A picked file as base64, for the `add_attachment` command. */
async function readFile(file: File): Promise<string> {
  const bytes = new Uint8Array(await file.arrayBuffer());
  let binary = "";
  for (let i = 0; i < bytes.length; i += 0x8000) {
    binary += String.fromCodePoint(...bytes.subarray(i, i + 0x8000));
  }
  return btoa(binary);
}

const commitField = (h: RecipientHandle | null, v: Addr[]) => (h ? h.commit() : v);

function AttachmentList({
  attachments,
  onRemove,
}: {
  attachments: DraftAttachment[];
  onRemove: (id: string) => void;
}) {
  return (
    <div className="mt-3 flex flex-wrap gap-2">
      {attachments.map((a) => (
        <span
          key={a.id}
          className="flex items-center gap-1.5 rounded-control border border-border-2 px-2 py-1 text-row"
        >
          <Paperclip size={12} strokeWidth={1.5} className="text-text-3" />
          <span className="max-w-[220px] truncate">{a.filename}</span>
          <button
            tabIndex={-1}
            className="text-text-3 hover:text-text"
            onClick={() => onRemove(a.id)}
          >
            <X size={12} />
          </button>
        </span>
      ))}
    </div>
  );
}

/** Send on the left, quiet icons on the right, under a hairline. */
function Footer({ blurred, onAttach }: { blurred: boolean; onAttach: () => void }) {
  return (
    <div className="mt-6 flex items-center gap-5 border-t border-border-2 pt-3 text-row">
      <button
        tabIndex={-1}
        onClick={sendOnScreen}
        className="font-medium text-text hover:text-accent"
        title="Send (⌘↵)"
      >
        Send
      </button>
      <span className="text-text-3">{blurred ? "Enter to edit · Esc to close" : "⌘↵ to send"}</span>
      <div className="flex-1" />
      <button
        tabIndex={-1}
        onClick={onAttach}
        className="text-text-3 hover:text-text-2"
        title="Attach (⌘⇧A)"
      >
        <Paperclip size={16} strokeWidth={1.5} />
      </button>
      <button
        tabIndex={-1}
        onClick={discardOnScreen}
        className="text-text-3 hover:text-text-2"
        title="Discard (⌘⇧,)"
      >
        <Trash2 size={16} strokeWidth={1.5} />
      </button>
    </div>
  );
}

export function Composer({ draft, inline }: { draft: string; inline: boolean }) {
  const state = useDraft(draft);
  const app = useAppState();
  const blurred = useChrome((s) => s.composeBlurred);
  const chromeAccount = useChrome((s) => s.account);
  const [fields, setFields] = useState<DraftFields>({
    from: null,
    to: [],
    cc: [],
    bcc: [],
    subject: "",
    bodyHtml: "",
    attachments: [],
  });
  const [loaded, setLoaded] = useState(false);
  const [showCc, setShowCc] = useState(false);
  const [showBcc, setShowBcc] = useState(false);
  const [showSubject, setShowSubject] = useState(!inline);
  const [link, setLink] = useState<string | null>(null);
  const dirty = useRef(false);
  const latest = useRef(fields);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const root = useRef<HTMLDivElement>(null);
  const toRef = useRef<RecipientHandle>(null);
  const ccRef = useRef<RecipientHandle>(null);
  const bccRef = useRef<RecipientHandle>(null);
  const subjectRef = useRef<HTMLInputElement>(null);
  const fileRef = useRef<HTMLInputElement>(null);
  const linkRef = useRef<HTMLInputElement>(null);

  const scheduleSave = useCallback(() => {
    dirty.current = true;
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => {
      timer.current = null;
      const pending = openComposer.pending();
      if (pending) act({ type: "saveDraft", draft, save: { type: "update", fields: pending } });
    }, SAVE_DELAY);
  }, [draft]);

  const update = useCallback(
    (patch: Partial<DraftFields>) => {
      latest.current = { ...latest.current, ...patch };
      setFields((f) => ({ ...f, ...patch }));
      scheduleSave();
    },
    [scheduleSave],
  );

  const attach = useCallback(() => fileRef.current?.click(), []);

  const editor = useEditor({
    extensions: [
      StarterKit.configure({
        // Removed: they clash with the compose table (Cmd+Shift+B, Cmd+Shift+S, Cmd+Enter).
        blockquote: false,
        strike: false,
        hardBreak: false,
        heading: false,
        codeBlock: false,
        code: false,
        horizontalRule: false,
        link: { openOnClick: false, autolink: true },
      }),
      StrikeKeys,
      HardBreakKeys,
      Placeholder.configure({ placeholder: inline ? "Write a reply…" : "Write a message…" }),
    ],
    content: "",
    autofocus: inline ? "end" : false,
    editorProps: {
      handleKeyDown: (_view, e) => {
        if (e.metaKey && e.key === "Enter") {
          e.preventDefault();
          sendOnScreen();
          return true;
        }
        return false;
      },
    },
    onUpdate: ({ editor: ed }) => {
      const html = ed.getHTML();
      update({ bodyHtml: html === "<p></p>" ? "" : html });
    },
    onFocus: () => useChrome.getState().set({ composeBlurred: false }),
  });

  // The first load carries the filled fields (adjusted during render, once).
  if (state && !state.gone && !loaded) {
    setLoaded(true);
    setFields((f) => ({
      from: state.from,
      to: state.to,
      cc: state.cc,
      bcc: state.bcc,
      subject: state.subject,
      bodyHtml: f.bodyHtml || state.bodyHtml,
      attachments: state.attachments,
    }));
    if (state.cc.length > 0) setShowCc(true);
    if (state.bcc.length > 0) setShowBcc(true);
    if (state.kind === "new") setShowSubject(true);
  }
  useEffect(() => {
    latest.current = fields;
  }, [fields]);
  // A resumed draft fills the editor.
  const firstBody = loaded ? (state?.bodyHtml ?? "") : "";
  useEffect(() => {
    if (editor?.isEmpty && firstBody) editor.commands.setContent(firstBody, { emitUpdate: false });
  }, [editor, firstBody]);

  // Register with the session so send, discard, and close carry the pending save.
  useEffect(() => {
    openComposer.draft = draft;
    openComposer.pending = () => {
      if (!dirty.current) return null;
      if (timer.current) {
        clearTimeout(timer.current);
        timer.current = null;
      }
      dirty.current = false;
      const f = latest.current;
      return {
        ...f,
        to: commitField(toRef.current, f.to),
        cc: commitField(ccRef.current, f.cc),
        bcc: commitField(bccRef.current, f.bcc),
      };
    };
    openComposer.invalid = () =>
      [...latest.current.to, ...latest.current.cc, ...latest.current.bcc]
        .filter((a) => !isValidEmail(a.email))
        .map((a) => a.email);
    openComposer.focus = () => editor?.commands.focus("end");
    openComposer.setFrom = (from) => update({ from });
    openComposer.attach = attach;
    return () => {
      if (openComposer.draft === draft) openComposer.draft = null;
    };
  }, [draft, editor, update, attach]);

  // A reply scrolls fully into view with the editor focused; a new message starts in To.
  useEffect(() => {
    if (inline) root.current?.scrollIntoView({ block: "nearest" });
    else requestAnimationFrame(() => toRef.current?.focus());
  }, [inline]);

  // Compose shortcuts work focused or blurred (SPEC Compose).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (useChrome.getState().palette) return;
      const blurredNow = useChrome.getState().composeBlurred;
      if (e.metaKey && e.shiftKey) {
        const k = e.key.toLowerCase();
        const run: Record<string, () => void> = {
          o: () => toRef.current?.focus(),
          c: () => {
            setShowCc(true);
            requestAnimationFrame(() => ccRef.current?.focus());
          },
          b: () => {
            setShowBcc(true);
            requestAnimationFrame(() => bccRef.current?.focus());
          },
          f: () => useChrome.getState().set({ palette: { kind: "from", draft } }),
          s: () => {
            setShowSubject(true);
            requestAnimationFrame(() => subjectRef.current?.focus());
          },
          m: () => editor?.commands.focus("end"),
          a: attach,
          ",": discardOnScreen,
          "<": discardOnScreen,
        };
        const f = run[k];
        if (f) {
          e.preventDefault();
          e.stopPropagation();
          f();
          return;
        }
      }
      if (e.metaKey && e.key === "Enter") {
        e.preventDefault();
        sendOnScreen();
        return;
      }
      if (e.metaKey && e.key.toLowerCase() === "k") {
        e.preventDefault();
        if (!blurredNow && editor?.isFocused) {
          const href: unknown = editor.getAttributes("link").href;
          setLink(typeof href === "string" ? href : "");
          requestAnimationFrame(() => linkRef.current?.focus());
        } else {
          useChrome.getState().set({ palette: { kind: "commands", draftActions: true } });
        }
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        if (link !== null) {
          setLink(null);
          editor?.commands.focus();
          return;
        }
        if (!blurredNow) {
          // The first Esc blurs the editor and leaves compose on screen.
          if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
          useChrome.getState().set({ composeBlurred: true });
        } else {
          closeComposerOnScreen();
        }
        return;
      }
      if (blurredNow) {
        if (e.key === "Enter") {
          e.preventDefault();
          editor?.commands.focus("end");
        } else if (!e.metaKey) {
          // Other keys do nothing while compose is blurred.
          e.preventDefault();
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [draft, editor, attach, link]);

  const onFiles = async (files: FileList | null) => {
    if (!files) return;
    const results = await Promise.all(
      Array.from(files).map(async (f): Promise<DraftAttachment | null> => {
        try {
          return await addAttachment(draft, f.name, f.type, await readFile(f));
        } catch (err) {
          toast.error(`Couldn't attach ${f.name}: ${String(err)}`);
          return null;
        }
      }),
    );
    const added = results.filter((a): a is DraftAttachment => a !== null);
    update({ attachments: [...latest.current.attachments, ...added] });
  };

  const account = state?.account ?? chromeAccount ?? 0;
  const sendAs = app?.accounts.find((a) => a.id === account)?.sendAs ?? [];

  const ccToggle =
    !showCc || !showBcc ? (
      <button
        tabIndex={-1}
        title="Cc and Bcc (⌘⇧C, ⌘⇧B)"
        className="flex h-6 w-6 items-center justify-center rounded-control text-text-3 hover:bg-fill-half hover:text-text-2"
        onClick={() => {
          setShowCc(true);
          setShowBcc(true);
          requestAnimationFrame(() => ccRef.current?.focus());
        }}
      >
        <ChevronDown size={15} strokeWidth={1.5} />
      </button>
    ) : null;

  return (
    <div
      ref={root}
      // A floating card, as Superhuman draws its composer.
      className={`selectable flex flex-col rounded-[10px] bg-bg px-10 pt-4 pb-3 shadow-[0_6px_28px_rgb(20_20_19/0.09),0_0_0_1px_rgb(20_20_19/0.05)] ${
        inline ? "mt-6 min-h-[220px]" : "min-h-[440px]"
      } ${blurred ? "opacity-95" : ""}`}
      onFocusCapture={() => useChrome.getState().set({ composeBlurred: false })}
    >
      {sendAs.length > 1 && (
        <div className="flex min-h-10 items-center text-body">
          <span className="w-20 shrink-0 text-text">From</span>
          <button
            tabIndex={-1}
            className="px-1 text-text-2 hover:text-text"
            onClick={() => useChrome.getState().set({ palette: { kind: "from", draft } })}
          >
            {fields.from ? formatAddr(fields.from) : ""}
          </button>
        </div>
      )}
      <RecipientField
        ref={toRef}
        label="To"
        account={account}
        value={fields.to}
        onChange={(to) => update({ to })}
        right={ccToggle}
        onTab={(shift) => {
          if (shift) return;
          if (showCc) ccRef.current?.focus();
          else if (showSubject) subjectRef.current?.focus();
          else editor?.commands.focus();
        }}
      />
      {showCc && (
        <RecipientField
          ref={ccRef}
          label="Cc"
          account={account}
          value={fields.cc}
          onChange={(cc) => update({ cc })}
        />
      )}
      {showBcc && (
        <RecipientField
          ref={bccRef}
          label="Bcc"
          account={account}
          value={fields.bcc}
          onChange={(bcc) => update({ bcc })}
        />
      )}
      {showSubject && (
        <input
          ref={subjectRef}
          className="mt-2 h-10 w-full bg-transparent text-body outline-none placeholder:text-text-3"
          placeholder="Subject"
          value={fields.subject}
          onChange={(e) => update({ subject: e.target.value })}
          onKeyDown={(e) => {
            if (e.key === "Tab" && !e.shiftKey) {
              e.preventDefault();
              editor?.commands.focus();
            }
          }}
        />
      )}
      <div className="relative pt-3">
        <EditorContent editor={editor} />
        {link !== null && (
          <div className="absolute top-0 left-0 z-10 flex items-center gap-2 rounded-control bg-bg p-1 shadow-float">
            <input
              ref={linkRef}
              className="w-72 rounded-control border border-border px-2 py-1 text-row outline-none"
              placeholder="https://"
              value={link}
              onChange={(e) => setLink(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  const href = link.trim();
                  const chain = editor?.chain().focus().extendMarkRange("link");
                  if (href)
                    chain
                      ?.setLink({ href: /^[a-z]+:/i.test(href) ? href : `https://${href}` })
                      .run();
                  else chain?.unsetLink().run();
                  setLink(null);
                }
              }}
            />
          </div>
        )}
      </div>
      {state?.quoteHtml && <QuoteBlock html={state.quoteHtml} />}
      {fields.attachments.length > 0 && (
        <AttachmentList
          attachments={fields.attachments}
          onRemove={(id) =>
            update({ attachments: latest.current.attachments.filter((x) => x.id !== id) })
          }
        />
      )}
      {/* Empty room in the card goes below the quote, as in Superhuman. */}
      <div className="flex-1" />
      <input
        ref={fileRef}
        type="file"
        multiple
        hidden
        onChange={(e) => void onFiles(e.target.files)}
      />
      <Footer blurred={blurred} onAttach={attach} />
    </div>
  );
}
