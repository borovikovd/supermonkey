import { HardBreak } from "@tiptap/extension-hard-break";
import { Strike } from "@tiptap/extension-strike";
import { Placeholder } from "@tiptap/extensions";
import { type Editor, EditorContent, useEditor } from "@tiptap/react";
import { StarterKit } from "@tiptap/starter-kit";
import { ChevronDown, Paperclip, Trash2, X } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";

import { act, later } from "../ipc/bridge";
import type { Addr, DraftAttachment, DraftFields } from "../ipc/types";
import { command, keyText, shortHintOf, useCommand } from "../keyboard/table";
import { openPalette } from "../navigation";
import { useChrome } from "../store/chrome";
import { useAppState, useDraft } from "../store/hooks";
import { UntrustedFrame } from "../ui/UntrustedFrame";
import { RecipientField, formatAddr, isValidEmail, type RecipientHandle } from "./RecipientField";
import {
  attachFiles,
  composeBlurred,
  composerFocused,
  focusNextFrame,
  leaveGoneDraft,
  loadedDraft,
  registerComposer,
  type Attaching,
  type ComposeField,
  type UpdateSave,
  useComposeState,
} from "./session";
import { takeTypedAhead } from "./start";

const SAVE_DELAY = 500;

const NONE: readonly Attaching[] = [];

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

function focusEditor(editor: Editor | null, atEnd = false) {
  if (!editor) return;
  if (atEnd) editor.commands.setTextSelection(editor.state.doc.content.size);
  editor.view.focus();
  editor.commands.scrollIntoView();
}

const QUOTE_STYLE = "body{margin:0;font:13px/20px var(--font-sans);color:var(--color-text-2)}";

function QuoteBlock({ html }: { html: string }) {
  const [open, setOpen] = useState(false);
  const remoteImages = useAppState()?.settings.remoteImages ?? true;
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
        <UntrustedFrame
          title="quoted"
          html={html}
          css={QUOTE_STYLE}
          remoteImages={remoteImages}
          className="mt-2 w-full border-0"
        />
      )}
    </div>
  );
}

function withLoaded(typed: DraftFields, loaded: DraftFields): DraftFields {
  const ids = new Set(loaded.attachments.map((a) => a.id));
  return {
    from: typed.from ?? loaded.from,
    to: typed.to.length > 0 ? typed.to : loaded.to,
    cc: typed.cc.length > 0 ? typed.cc : loaded.cc,
    bcc: typed.bcc.length > 0 ? typed.bcc : loaded.bcc,
    subject: typed.subject || loaded.subject,
    bodyHtml: loaded.bodyHtml + typed.bodyHtml,
    attachments: [...loaded.attachments, ...typed.attachments.filter((a) => !ids.has(a.id))],
  };
}

function AttachmentList({
  attachments,
  attaching,
  onRemove,
}: {
  attachments: DraftAttachment[];
  attaching: readonly Attaching[];
  onRemove: (id: string) => void;
}) {
  return (
    <div className="mt-3 flex flex-wrap gap-2">
      {attachments.map((a) => (
        <span
          key={a.id}
          className="flex items-center gap-1.5 rounded-md border border-border-2 px-2 py-1 text-row"
        >
          <Paperclip size={12} strokeWidth={1.5} className="text-text-3" />
          <span className="max-w-filename truncate">{a.filename}</span>
          <button
            tabIndex={-1}
            aria-label={`Remove ${a.filename}`}
            className="text-text-3 hover:text-text"
            onClick={() => onRemove(a.id)}
          >
            <X size={12} />
          </button>
        </span>
      ))}
      {attaching.map((a) => (
        <span
          key={`attaching-${a.key}`}
          aria-busy="true"
          className="flex items-center gap-1.5 rounded-md border border-dashed border-border-2 px-2 py-1 text-row text-text-3"
        >
          <Paperclip size={12} strokeWidth={1.5} />
          <span className="max-w-filename truncate">{a.name}</span>
        </span>
      ))}
    </div>
  );
}

function Footer({ blurred }: { blurred: boolean }) {
  const send = useCommand("send");
  const attach = useCommand("attach");
  const discard = useCommand("discard");
  const hint = (blurred ? (["compose-edit", "compose-escape"] as const) : (["send"] as const))
    .map((id) => shortHintOf(command(id)))
    .join(" · ");
  return (
    <div className="mt-6 flex items-center gap-5 border-t border-border-2 pt-3 text-row">
      <button
        tabIndex={-1}
        onClick={send.run}
        className="font-medium text-text hover:text-accent"
        title={send.title}
      >
        Send
      </button>
      <span className="text-text-3">{hint}</span>
      <div className="flex-1" />
      <button
        tabIndex={-1}
        onClick={attach.run}
        className="text-text-3 hover:text-text-2"
        title={attach.title}
      >
        <Paperclip size={16} strokeWidth={1.5} />
      </button>
      <button
        tabIndex={-1}
        onClick={discard.run}
        className="text-text-3 hover:text-text-2"
        title={discard.title}
      >
        <Trash2 size={16} strokeWidth={1.5} />
      </button>
    </div>
  );
}

export function Composer({ draft, inline }: { draft: string; inline: boolean }) {
  const state = useDraft(draft);
  const app = useAppState();
  const blurred = useComposeState((s) => s.blurred);
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
  const [loadedBody, setLoadedBody] = useState("");
  const [showCc, setShowCc] = useState(false);
  const [showBcc, setShowBcc] = useState(false);
  const [showSubject, setShowSubject] = useState(!inline);
  const [link, setLink] = useState<string | null>(null);
  const dirty = useRef(false);
  const loadedRef = useRef(false);
  const latest = useRef(fields);
  const timer = useRef<(() => void) | null>(null);
  const takePending = useRef<(finish: boolean) => UpdateSave | null>(() => null);
  const seen = useRef({ rev: 0, ids: new Set<string>() });
  const root = useRef<HTMLDivElement>(null);
  const toRef = useRef<RecipientHandle>(null);
  const ccRef = useRef<RecipientHandle>(null);
  const bccRef = useRef<RecipientHandle>(null);
  const subjectRef = useRef<HTMLInputElement>(null);
  const fileRef = useRef<HTMLInputElement>(null);
  const linkRef = useRef<HTMLInputElement>(null);
  const attaching = useComposeState((s) => s.attaching[draft] ?? NONE);

  const scheduleSave = useCallback(() => {
    dirty.current = true;
    if (!loadedRef.current) return;
    timer.current?.();
    timer.current = later(() => {
      timer.current = null;
      const save = takePending.current(false);
      if (save) act({ type: "saveDraft", draft, save });
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
    autofocus: false,
    onUpdate: ({ editor: ed }) => {
      const html = ed.getHTML();
      update({ bodyHtml: html === "<p></p>" ? "" : html });
    },
    onFocus: composerFocused,
  });

  if (state && !state.gone && !loaded) {
    setLoaded(true);
    setLoadedBody(state.bodyHtml);
    setFields((f) => withLoaded(f, state));
    if (state.cc.length > 0) setShowCc(true);
    if (state.bcc.length > 0) setShowBcc(true);
    if (state.kind === "new") setShowSubject(true);
  }
  const goneAtLoad = state?.gone === true && !loaded;
  useEffect(() => {
    if (goneAtLoad) leaveGoneDraft(draft);
  }, [goneAtLoad, draft]);
  useEffect(() => {
    latest.current = fields;
    for (const a of fields.attachments) seen.current.ids.add(a.id);
  }, [fields]);
  useEffect(() => {
    if (!loaded || !state || state.gone) return;
    const added = state.attachments.filter((a) => !seen.current.ids.has(a.id));
    seen.current.rev = state.rev;
    if (added.length === 0) return;
    for (const a of added) seen.current.ids.add(a.id);
    const attachments = [...latest.current.attachments, ...added];
    latest.current = { ...latest.current, attachments };
    setFields((f) => ({ ...f, attachments: [...f.attachments, ...added] }));
  }, [loaded, state]);
  useEffect(() => {
    if (!loaded || loadedRef.current) return;
    loadedRef.current = true;
    if (dirty.current) scheduleSave();
  }, [loaded, scheduleSave]);
  useEffect(() => {
    if (!editor || !loadedBody) return;
    if (editor.isEmpty) editor.commands.setContent(loadedBody, { emitUpdate: false });
    else editor.commands.insertContentAt(0, loadedBody);
  }, [editor, loadedBody]);

  useEffect(() => {
    takePending.current = (finish) => {
      const commit = (h: RecipientHandle | null, v: Addr[]) => (finish && h ? h.commit() : v);
      const to = commit(toRef.current, latest.current.to);
      const cc = commit(ccRef.current, latest.current.cc);
      const bcc = commit(bccRef.current, latest.current.bcc);
      if (!dirty.current) return null;
      const typed = { ...latest.current, to, cc, bcc };
      let save: UpdateSave = { type: "update", fields: typed, rev: seen.current.rev };
      if (!loadedRef.current) {
        const first = loadedDraft(draft);
        if (!first || first.gone) return null;
        save = { type: "update", fields: withLoaded(typed, first), rev: first.rev };
      }
      timer.current?.();
      timer.current = null;
      dirty.current = false;
      return save;
    };
    const focusLater = (el: () => { focus: () => void } | null) =>
      focusNextFrame(() => el()?.focus());
    const focus = (field: ComposeField) => {
      composerFocused();
      switch (field) {
        case "to":
          toRef.current?.focus();
          return;
        case "cc":
          setShowCc(true);
          focusLater(() => ccRef.current);
          return;
        case "bcc":
          setShowBcc(true);
          focusLater(() => bccRef.current);
          return;
        case "subject":
          setShowSubject(true);
          focusLater(() => subjectRef.current);
          return;
        case "body":
          focusEditor(editor, true);
      }
    };
    return registerComposer({
      draft,
      pending: () => takePending.current(true),
      commitTyped: () => {
        for (const field of [toRef, ccRef, bccRef]) field.current?.commit();
      },
      invalid: () =>
        [...latest.current.to, ...latest.current.cc, ...latest.current.bcc]
          .filter((a) => !isValidEmail(a.email))
          .map((a) => a.email),
      addAttachments: (added) => update({ attachments: [...latest.current.attachments, ...added] }),
      setFrom: (from) => update({ from }),
      attach,
      focus,
      editorFocused: () => editor?.isFocused ?? false,
      openLink: () => {
        const href: unknown = editor?.getAttributes("link").href;
        setLink(typeof href === "string" ? href : "");
        focusLater(() => linkRef.current);
      },
      closeLink: () => {
        if (!linkRef.current) return false;
        setLink(null);
        focusEditor(editor);
        return true;
      },
    });
  }, [draft, editor, update, attach]);

  useEffect(() => {
    if (!editor || !inline) return;
    const text = takeTypedAhead();
    if (text)
      editor.chain().setTextSelection(editor.state.doc.content.size).insertContent(text).run();
  }, [editor, inline]);

  useEffect(() => {
    if (composeBlurred()) return;
    if (inline) {
      editor?.view.focus();
      root.current?.scrollIntoView({ block: "nearest" });
    } else toRef.current?.focus();
  }, [editor, inline]);

  useEffect(() => {
    const el = root.current;
    const content = el?.parentElement;
    const scroller = content?.parentElement;
    if (!inline || !el || !content || !scroller) return;
    let top = el.offsetTop;
    const follow = new ResizeObserver(() => {
      const moved = el.offsetTop - top;
      top = el.offsetTop;
      if (moved === 0 || !el.contains(document.activeElement)) return;
      const view = scroller.getBoundingClientRect();
      const was = el.getBoundingClientRect().top - moved;
      if (was < view.bottom && was + el.offsetHeight > view.top) scroller.scrollTop += moved;
    });
    follow.observe(content);
    return () => follow.disconnect();
  }, [inline]);

  const onFiles = (picker: HTMLInputElement) => {
    const files = Array.from(picker.files ?? []);
    picker.value = "";
    if (files.length > 0) attachFiles(draft, files);
  };

  const account = state?.account ?? chromeAccount ?? 0;
  const sendAs = app?.accounts.find((a) => a.id === account)?.sendAs ?? [];

  const ccToggle =
    !showCc || !showBcc ? (
      <button
        tabIndex={-1}
        title={`Cc and Bcc (${keyText(command("focus-cc"))}, ${keyText(command("focus-bcc"))})`}
        className="flex h-6 w-6 items-center justify-center rounded-md text-text-3 hover:bg-fill-half hover:text-text-2"
        onClick={() => {
          setShowCc(true);
          setShowBcc(true);
          focusNextFrame(() => ccRef.current?.focus());
        }}
      >
        <ChevronDown size={15} strokeWidth={1.5} />
      </button>
    ) : null;

  return (
    <div
      ref={root}
      className={`selectable flex flex-col rounded-xl bg-bg px-10 pt-4 pb-3 shadow-raised ${
        inline ? "-mx-10 mt-6 min-h-composer-inline-min" : "min-h-composer-min"
      } ${blurred ? "opacity-95" : ""}`}
      onFocusCapture={composerFocused}
    >
      {sendAs.length > 1 && (
        <div className="flex min-h-10 items-center text-body">
          <span className="w-20 shrink-0 text-text">From</span>
          <button
            tabIndex={-1}
            className="px-1 text-text-2 hover:text-text"
            onClick={() => openPalette({ kind: "from" })}
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
          else focusEditor(editor);
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
              focusEditor(editor);
            }
          }}
        />
      )}
      <div className="relative pt-3">
        <EditorContent editor={editor} />
        {link !== null && (
          <div className="absolute top-0 left-0 z-10 flex items-center gap-2 rounded-md bg-bg p-1 shadow-float">
            <input
              ref={linkRef}
              className="w-72 rounded-md border border-border px-2 py-1 text-row outline-none"
              placeholder="https://"
              value={link}
              onChange={(e) => setLink(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  const href = link.trim();
                  const chain = editor?.chain().extendMarkRange("link");
                  if (href)
                    chain
                      ?.setLink({ href: /^[a-z]+:/i.test(href) ? href : `https://${href}` })
                      .run();
                  else chain?.unsetLink().run();
                  setLink(null);
                  focusEditor(editor);
                }
              }}
            />
          </div>
        )}
      </div>
      {state?.quoteHtml && <QuoteBlock html={state.quoteHtml} />}
      {fields.attachments.length + attaching.length > 0 && (
        <AttachmentList
          attachments={fields.attachments}
          attaching={attaching}
          onRemove={(id) =>
            update({ attachments: latest.current.attachments.filter((x) => x.id !== id) })
          }
        />
      )}
      <div className="flex-1" />
      <input ref={fileRef} type="file" multiple hidden onChange={(e) => onFiles(e.target)} />
      <Footer blurred={blurred} />
    </div>
  );
}
