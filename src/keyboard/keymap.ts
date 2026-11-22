type Chord = { key: string; meta: boolean; shift: boolean; ctrl: boolean; alt: boolean };

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

const NAMED = new Set(["Enter", "Escape", "Tab", "ArrowDown", "ArrowUp", "Backspace"]);

const SHIFTED: Record<string, string> = { ",": "<" };

export function chordMatches(c: Chord, e: KeyboardEvent): boolean {
  if (e.metaKey !== c.meta || e.ctrlKey !== c.ctrl || e.altKey !== c.alt) return false;
  const modified = c.meta || c.ctrl || c.alt;
  if (c.key === "1…9") return /^Digit[1-9]$/.test(e.code);
  if (NAMED.has(c.key)) return e.key === c.key && e.shiftKey === c.shift;
  if (/^[a-z]$/i.test(c.key)) {
    const k = c.key.toLowerCase();
    if (modified) return e.key.toLowerCase() === k && e.shiftKey === c.shift;
    return e.key === (c.shift ? k.toUpperCase() : k);
  }
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

export function bindingChips(b: Binding): string[] {
  return b.flatMap((c) => [...mods(c), keyName(c.key, true)]);
}

export function bindingText(b: Binding): string {
  if (b.length > 1) return b.map((c) => c.key.toLowerCase()).join(" ");
  return b.map((c) => mods(c).join("") + keyName(c.key, false)).join("");
}
