import { Composer } from "../compose/Composer";

/** `c`: a new message that replaces the screen. */
export function ComposeScreen({ draft }: { draft: string }) {
  return (
    <div className="min-h-0 flex-1 overflow-y-auto">
      <div className="mx-auto w-full max-w-compose min-w-content-min px-12 pt-2.75 pb-24">
        <h1 className="mb-5 pl-10 text-subject">New Message</h1>
        <Composer key={draft} draft={draft} inline={false} />
      </div>
    </div>
  );
}
