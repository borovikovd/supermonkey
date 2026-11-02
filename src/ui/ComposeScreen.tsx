import { Composer } from "../compose/Composer";

/** `c`: a new message that replaces the screen. */
export function ComposeScreen({ draft }: { draft: string }) {
  return (
    <div className="min-h-0 flex-1 overflow-y-auto">
      <div className="mx-auto w-full max-w-[860px] min-w-[520px] px-12 pt-[11px] pb-24">
        <div className="mb-5 pl-10 text-subject font-semibold">New Message</div>
        <Composer key={draft} draft={draft} inline={false} />
      </div>
    </div>
  );
}
