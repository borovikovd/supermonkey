// A message frame's document can't see the app's CSS variables, so the
// tokens its CSS uses are copied in from the app's root once.

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

/** A `:root` rule declaring the design tokens frame CSS may use. */
export function frameTokens(): string {
  if (rule === undefined) {
    const root = getComputedStyle(document.documentElement);
    rule = `:root{${NAMES.map((name) => `${name}:${root.getPropertyValue(name).trim()}`).join(";")}}`;
  }
  return rule;
}
