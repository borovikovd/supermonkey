// To, Cc, and Bcc as chips with autocomplete (SPEC Recipients and autocomplete).

import { useEffect, useImperativeHandle, useLayoutEffect, useRef, useState } from "react";

import { autocomplete, later } from "../ipc/bridge";
import type { Addr, ContactSuggestion } from "../ipc/types";
import { registerKeys } from "../keyboard/dispatch";
import { measure } from "../keyboard/meter";

const utf8 = new TextEncoder();
const byteLength = (s: string) => utf8.encode(s).length;
// Rust's `char::is_whitespace`, `is_control`, and `is_alphanumeric`.
const BAD_LOCAL = /[\p{White_Space}\p{Cc}<>()[\],;:\\"@]/u;
const LABEL = /^[\p{Alphabetic}\p{N}-]+$/u;

/**
 * Whether the core would send to this address: `ub_mime::is_valid_email`,
 * ported rule for rule, so a chip shows in `danger` exactly when Send would
 * be refused. It runs on every chip and keystroke, so it can't wait on a
 * command. Both are held to one table of cases
 * (`src-tauri/crates/ub-mime/tests/fixtures/email-addresses.json`, RCP-11).
 */
export function isValidEmail(s: string): boolean {
  const at = s.lastIndexOf("@");
  if (at < 0) return false;
  const local = s.slice(0, at);
  const domain = s.slice(at + 1);
  if (
    byteLength(s) > 254 ||
    local === "" ||
    byteLength(local) > 64 ||
    BAD_LOCAL.test(local) ||
    local.startsWith(".") ||
    local.endsWith(".") ||
    local.includes("..")
  )
    return false;
  const labels = domain.split(".");
  const tld = labels.at(-1) ?? "";
  return (
    labels.length >= 2 &&
    labels.every(
      (label) =>
        label !== "" &&
        byteLength(label) <= 63 &&
        !label.startsWith("-") &&
        !label.endsWith("-") &&
        LABEL.test(label),
    ) &&
    // At least two characters (code points, as Rust counts them).
    /^.{2,}$/su.test(tld) &&
    !/^[0-9]*$/.test(tld)
  );
}

/** Parses `Name <a@b.c>` or a bare address. */
export function parseAddr(text: string): Addr | null {
  const t = text.trim().replace(/[,;]+$/, "");
  if (!t) return null;
  const m = /^"?([^"<]*?)"?\s*<([^>]+)>$/.exec(t);
  if (m?.[1] !== undefined && m[2] !== undefined)
    return { name: m[1].trim() || null, email: m[2].trim() };
  return { name: null, email: t };
}

/** `Name <a@b.c>`, or the bare address when there is no name. */
export const formatAddr = (a: Addr) => (a.name ? `${a.name} <${a.email}>` : a.email);

/** Splits pasted text into addresses (commas, semicolons, newlines). */
export function splitAddrs(text: string): Addr[] {
  const parts = text.split(/[\n;]+|,(?=(?:[^"]*"[^"]*")*[^"]*$)/);
  return parts.map(parseAddr).filter((a): a is Addr => a !== null);
}

export type RecipientHandle = { focus: () => void; commit: () => Addr[] };

function Highlight({ text, start, end }: { text: string; start: number; end: number }) {
  // Offsets are byte offsets from Rust; map through UTF-8.
  const bytes = new TextEncoder().encode(text);
  const dec = new TextDecoder();
  const a = dec.decode(bytes.slice(0, start));
  const b = dec.decode(bytes.slice(start, end));
  const c = dec.decode(bytes.slice(end));
  return (
    <>
      {a}
      <b className="font-semibold">{b}</b>
      {c}
    </>
  );
}

export function RecipientField({
  ref,
  label,
  account,
  value,
  onChange,
  onTab,
  right,
}: {
  ref?: React.Ref<RecipientHandle>;
  label: string;
  account: number;
  value: Addr[];
  onChange: (v: Addr[]) => void;
  onTab?: (shift: boolean) => void;
  /** Controls at the right end of the row (e.g. the Cc/Bcc toggle). */
  right?: React.ReactNode;
}) {
  const [text, setText] = useState("");
  const [suggestions, setSuggestions] = useState<ContactSuggestion[]>([]);
  const [active, setActive] = useState(0);
  const [selectedChip, setSelectedChip] = useState<number | null>(null);
  const input = useRef<HTMLInputElement>(null);
  const seq = useRef(0);

  const addAll = (addrs: Addr[]) => {
    const have = new Set(value.map((a) => a.email.toLowerCase()));
    const next = [...value];
    for (const a of addrs) {
      if (!have.has(a.email.toLowerCase())) {
        next.push(a);
        have.add(a.email.toLowerCase());
      }
    }
    onChange(next);
  };

  const clearInput = () => {
    // Suggestions still on their way are for text that is gone.
    seq.current++;
    setText("");
    setSuggestions([]);
  };

  /** Turns typed text into a chip; returns the full list. */
  const commit = (): Addr[] => {
    const a = parseAddr(text);
    clearInput();
    if (!a) return value;
    const next = value.some((x) => x.email.toLowerCase() === a.email.toLowerCase())
      ? value
      : [...value, a];
    onChange(next);
    return next;
  };

  useImperativeHandle(ref, () => ({ focus: () => input.current?.focus(), commit }));

  /** What leaving the field does, as of the latest render: a click that blurred it may change the chips first. */
  const onLeft = useRef(() => {});
  useLayoutEffect(() => {
    onLeft.current = () => {
      if (text.trim()) commit();
      setSuggestions([]);
    };
  });
  /** Cancels the pending `onLeft`. */
  const leaving = useRef<(() => void) | null>(null);
  useEffect(() => () => leaving.current?.(), []);

  const query = (q: string) => {
    const start = performance.now();
    const n = ++seq.current;
    if (!q.trim()) {
      setSuggestions([]);
      return;
    }
    void autocomplete(account, q).then((s) => {
      if (n !== seq.current) return;
      setSuggestions(s);
      setActive(0);
      measure("autocomplete", start);
    });
  };

  const accept = (s: ContactSuggestion) => {
    addAll([{ name: s.name || null, email: s.email }]);
    clearInput();
  };

  const open = suggestions.length > 0;
  // While the list is open and its field has focus, its keys come before the
  // composer's (src/keyboard/dispatch.ts). A field just left keeps its list a
  // moment for a click, but not the keys: they belong to where focus went.
  const [focused, setFocused] = useState(false);
  useEffect(() => {
    if (!open || !focused) return;
    return registerKeys("suggestions", (e) => {
      if (e.metaKey || e.ctrlKey || e.altKey) return false;
      if (e.key === "ArrowDown") setActive((a) => Math.min(a + 1, suggestions.length - 1));
      else if (e.key === "ArrowUp") setActive((a) => Math.max(a - 1, 0));
      else if (e.key === "Enter" || e.key === "Tab") {
        const pick = suggestions[active];
        if (pick) accept(pick);
      } else if (e.key === "Escape") {
        // Closes the list without leaving the field.
        setSuggestions([]);
      } else return false;
      e.preventDefault();
      return true;
    });
  });
  return (
    <div className="relative flex min-h-10 flex-wrap items-center gap-x-1 py-1 text-body">
      <span className="w-20 shrink-0 text-text">{label}</span>
      {value.map((a, i) => {
        const bad = !isValidEmail(a.email);
        return (
          <span
            key={a.email.toLowerCase()}
            title={a.email}
            data-chip={a.email}
            aria-invalid={bad || undefined}
            data-selected={selectedChip === i || undefined}
            className={`group flex items-center gap-0.5 rounded-md px-1 ${
              bad ? "text-danger" : "text-link"
            } ${selectedChip === i ? "bg-accent-soft" : ""}`}
          >
            {a.name || a.email}
            {i < value.length - 1 && <span className="text-text-3">,</span>}
            <button
              tabIndex={-1}
              aria-label={`Remove ${a.email}`}
              className="invisible text-text-3 group-hover:visible hover:text-text"
              onClick={() => onChange(value.filter((_, j) => j !== i))}
            >
              ×
            </button>
          </span>
        );
      })}
      <input
        ref={input}
        role="combobox"
        aria-label={label}
        aria-expanded={open}
        aria-controls={`${label}-suggestions`}
        aria-activedescendant={open ? `${label}-suggestion-${active}` : undefined}
        className="min-w-30 flex-1 bg-transparent px-1 outline-none"
        value={text}
        onChange={(e) => {
          const v = e.target.value;
          setSelectedChip(null);
          // A comma, or a space after a complete address, makes a chip.
          if (/[,;]$/.test(v) || (/\s$/.test(v) && isValidEmail(v.trim()))) {
            const a = parseAddr(v);
            if (a) addAll([a]);
            clearInput();
            return;
          }
          setText(v);
          query(v);
        }}
        onPaste={(e) => {
          const t = e.clipboardData.getData("text");
          if (/[,;\n]/.test(t) || t.includes("@")) {
            e.preventDefault();
            addAll(splitAddrs(text + t));
            clearInput();
          }
        }}
        onFocus={() => {
          setFocused(true);
          leaving.current?.();
          leaving.current = null;
        }}
        onBlur={() => {
          setFocused(false);
          // Leaving the field turns typed text into a chip.
          leaving.current?.();
          leaving.current = later(() => {
            leaving.current = null;
            onLeft.current();
          }, 120);
        }}
        onKeyDown={(e) => {
          if (e.key === "Backspace" && text === "" && value.length > 0) {
            e.preventDefault();
            if (selectedChip === null) setSelectedChip(value.length - 1);
            else {
              onChange(value.filter((_, j) => j !== selectedChip));
              setSelectedChip(null);
            }
            return;
          }
          if ((e.key === "Tab" || e.key === "Enter") && !e.metaKey) {
            if (text.trim()) {
              e.preventDefault();
              commit();
              return;
            }
            if (e.key === "Tab" && onTab) {
              e.preventDefault();
              onTab(e.shiftKey);
            }
          }
        }}
      />
      {right && <div className="ml-auto flex items-center gap-1 self-start pt-1">{right}</div>}
      {open && (
        // The combobox pattern: focus stays in the input (aria-activedescendant),
        // and suggestions show name and address, which a native <datalist> cannot.
        <div
          // oxlint-disable-next-line jsx-a11y/prefer-tag-over-role -- rich rows; native elements cannot render them
          role="listbox"
          id={`${label}-suggestions`}
          className="fade-in absolute top-full left-20 z-20 mt-1 w-menu overflow-hidden rounded-lg bg-bg py-1 shadow-float"
        >
          {suggestions.map((s, i) => (
            // oxlint-disable-next-line jsx-a11y/interactive-supports-focus -- see the listbox
            <div
              key={s.email}
              // oxlint-disable-next-line jsx-a11y/prefer-tag-over-role -- rich rows; native elements cannot render them
              role="option"
              id={`${label}-suggestion-${i}`}
              aria-selected={i === active}
              onMouseDown={(e) => {
                e.preventDefault();
                accept(s);
              }}
              className={`flex h-row items-center gap-2 px-3 ${i === active ? "bg-accent-soft" : ""}`}
            >
              <span className="truncate font-medium">
                {s.matchField === "name" ? (
                  <Highlight text={s.name} start={s.matchStart} end={s.matchEnd} />
                ) : (
                  s.name
                )}
              </span>
              <span className="truncate text-text-2">
                {s.matchField === "email" ? (
                  <Highlight text={s.email} start={s.matchStart} end={s.matchEnd} />
                ) : (
                  s.email
                )}
              </span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
