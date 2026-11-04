/** Search operators, as Superhuman lists them beside the results. */
const TIPS: [string, string][] = [
  ["from:nicole", "from Nicole"],
  ["to:roman", "to Roman"],
  ['"be brilliant"', 'contains "be brilliant"'],
  ["subject:lunch", 'subject contains "lunch"'],
  ["has:attachment", "with attachments"],
  ["label:travel", "with the label Travel"],
  ["in:sent", "in Sent"],
  ["-in:inbox", "not in the Inbox"],
  ["is:unread", "unread conversations"],
  ["is:starred", "starred conversations"],
  ["after:2026/01/31", "after a date"],
  ["lunch OR dinner", "either word"],
  ["-newsletter", "without the word"],
];

export function SearchTips() {
  return (
    <aside
      aria-label="Search tips"
      className="hidden w-[260px] shrink-0 flex-col gap-4 overflow-y-auto border-l border-border-2 px-6 py-4 lg:flex"
    >
      <h2 className="text-row font-semibold text-text">Tips</h2>
      <dl className="flex flex-col gap-3">
        {TIPS.map(([op, what]) => (
          <div key={op}>
            <dt className="text-row text-text">{op}</dt>
            <dd className="text-row text-text-2">{what}</dd>
          </div>
        ))}
      </dl>
    </aside>
  );
}
