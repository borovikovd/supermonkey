import { Composer } from "../compose/Composer";

export function InlineComposer({ draft }: { draft: string }) {
  return <Composer key={draft} draft={draft} inline />;
}
