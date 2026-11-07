// Key bindings, written as `e`, `Shift+E`, `Meta+Shift+,`, `Control+1…9`, or
// a sequence such as `g e`: matched against keydown events, and shown as
// palette chips or as text for the shortcut sheet, hints, and titles.

type Chord = { key: string; meta: boolean; shift: boolean; ctrl: boolean; alt: boolean };

/** One chord, or two pressed one after the other. */
export type Binding = readonly Chord[];

export function parseBinding(spec: string): Binding {
  return spec.split(" ").map((chord) => {
    const parts = chord.split("+");
    const key = parts.pop() ?? "";
    const mods = new Set(parts);
    return {
      key,
      meta: mods.has("Meta"),
      shift: mods.has("Shift"),
      ctrl: mods.has("Control"),
      alt: mods.has("Alt"),
    };
  });
}

/** Keys named by `KeyboardEvent.key` rather than typed. */
const NAMED = new Set(["Enter", "Escape", "Tab", "ArrowDown", "ArrowUp", "Backspace"]);

/** What a US layout types for a symbol with Shift held, which WebKit reports with Cmd. */
const SHIFTED: Record<string, string> = { ",": "<" };

export function chordMatches(c: Chord, e: KeyboardEvent): boolean {
  if (e.metaKey !== c.meta || e.ctrlKey !== c.ctrl || e.altKey !== c.alt) return false;
  const modified = c.meta || c.ctrl || c.alt;
  if (c.key === "1…9") return /^Digit[1-9]$/.test(e.code);
  if (NAMED.has(c.key)) return e.key === c.key && e.shiftKey === c.shift;
  if (/^[a-z]$/i.test(c.key)) {
    const k = c.key.toLowerCase();
    if (modified) return e.key.toLowerCase() === k && e.shiftKey === c.shift;
    // A letter alone: Shift makes it a capital, and so does Caps Lock.
    return e.key === (c.shift ? k.toUpperCase() : k);
  }
  // A symbol is the character typed, whatever Shift took to type it.
  if (modified && e.shiftKey !== c.shift) return false;
  return e.key === c.key || (c.shift && e.key === SHIFTED[c.key]);
}

const MODS: [keyof Chord, string][] = [
  ["ctrl", "⌃"],
  ["alt", "⌥"],
  ["meta", "⌘"],
  ["shift", "⇧"],
];

const mods = (c: Chord) => MODS.filter(([m]) => c[m]).map(([, sign]) => sign);

function keyName(key: string, chip: boolean): string {
  switch (key) {
    case "Enter":
      return chip ? "↵" : "Enter";
    case "Escape":
      return "Esc";
    case "ArrowDown":
      return "↓";
    case "ArrowUp":
      return "↑";
    case "Tab":
    case "Backspace":
      return key;
  }
  return key.toUpperCase();
}

/** The palette's chips: one per modifier and key, e.g. `⌘ ⇧ A` or `G E`. */
export function bindingChips(b: Binding): string[] {
  return b.flatMap((c) => [...mods(c), keyName(c.key, true)]);
}

/** As text: `E`, `⇧E`, `⌘Enter`, `⌃1…9`, or a sequence as typed, `g e`. */
export function bindingText(b: Binding): string {
  if (b.length > 1) return b.map((c) => c.key.toLowerCase()).join(" ");
  return b.map((c) => mods(c).join("") + keyName(c.key, false)).join("");
}
