const NAMES = [
  "--color-text",
  "--color-text-2",
  "--color-text-3",
  "--color-link",
  "--color-border-2",
  "--color-accent",
  "--font-sans",
] as const;

let rule: string | undefined;

export function frameTokens(): string {
  if (rule === undefined) {
    const root = getComputedStyle(document.documentElement);
    rule = `:root{${NAMES.map((name) => `${name}:${root.getPropertyValue(name).trim()}`).join(";")}}`;
  }
  return rule;
}
