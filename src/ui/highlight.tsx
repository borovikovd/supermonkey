export function searchTerms(query: string): string[] {
  const out: string[] = [];
  for (const m of query.matchAll(/"([^"]+)"|(\S+)/g)) {
    const [, phrase, word] = m;
    if (phrase) out.push(phrase);
    else if (word && !word.includes(":") && !word.startsWith("-") && !/^(or|and)$/i.test(word)) {
      const bare = word.replace(/[()]/g, "");
      if (bare) out.push(bare);
    }
  }
  return out;
}

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
