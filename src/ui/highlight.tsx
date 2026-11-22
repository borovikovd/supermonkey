const escape = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

export function marked(text: string, terms: readonly string[]): React.ReactNode {
  if (terms.length === 0 || !text) return text;
  const parts = text.split(new RegExp(`(${terms.map(escape).join("|")})`, "gi"));
  let start = 0;
  return parts.map((part, i) => {
    const at = start;
    start += part.length;
    return i % 2 === 1 ? (
      <mark key={at} className="rounded-xs bg-match text-inherit">
        {part}
      </mark>
    ) : (
      part
    );
  });
}
