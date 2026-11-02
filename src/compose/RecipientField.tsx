// To, Cc, and Bcc as chips with autocomplete (SPEC Recipients and autocomplete).

import { useImperativeHandle, useRef, useState } from "react";

import { autocomplete } from "../ipc/bridge";
import type { Addr, ContactSuggestion } from "../ipc/types";
import { measure } from "../keyboard/meter";

const EMAIL = /^[^\s@<>(),;:"]+@[^\s@<>(),;:"]+\.[^\s@<>(),;:"]+$/;
export const isValidEmail = (s: string) => EMAIL.test(s.trim());

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
  return (
    <div className="relative flex min-h-10 flex-wrap items-center gap-x-1 py-1 text-body">
      <span className="w-20 shrink-0 text-text">{label}</span>
      {value.map((a, i) => {
        const bad = !isValidEmail(a.email);
        return (
          <span
            key={a.email.toLowerCase()}
            title={a.email}
            className={`group flex items-center gap-0.5 rounded-control px-1 ${
              bad ? "text-danger" : "text-link"
            } ${selectedChip === i ? "bg-accent-soft" : ""}`}
          >
            {a.name || a.email}
            {i < value.length - 1 && <span className="text-text-3">,</span>}
            <button
              tabIndex={-1}
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
        className="min-w-[120px] flex-1 bg-transparent px-1 outline-none"
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
        onBlur={() => {
          // Leaving the field turns typed text into a chip.
          setTimeout(() => {
            if (text.trim()) commit();
            setSuggestions([]);
          }, 120);
        }}
        onKeyDown={(e) => {
          if (open) {
            if (e.key === "ArrowDown") {
              e.preventDefault();
              setActive((a) => Math.min(a + 1, suggestions.length - 1));
              return;
            }
            if (e.key === "ArrowUp") {
              e.preventDefault();
              setActive((a) => Math.max(a - 1, 0));
              return;
            }
            if (e.key === "Enter" || e.key === "Tab") {
              e.preventDefault();
              e.stopPropagation();
              const pick = suggestions[active];
              if (pick) accept(pick);
              return;
            }
            if (e.key === "Escape") {
              // Closes the list without leaving the field.
              e.preventDefault();
              e.stopPropagation();
              setSuggestions([]);
              return;
            }
          }
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
          className="fade-in absolute top-full left-20 z-20 mt-1 w-[420px] overflow-hidden rounded-sheet bg-bg py-1 shadow-float"
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
