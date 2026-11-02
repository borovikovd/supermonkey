import { useRef, useState } from "react";

import { act, signInAgain } from "../ipc/bridge";
import type { AccountInfo, SplitInfo } from "../ipc/types";
import { useChrome } from "../store/chrome";
import { useAppState, useCurrent, useSummary } from "../store/hooks";

const SHORTCUTS: [string, [string, string][]][] = [
  [
    "Navigate",
    [
      ["J / K", "Next / previous conversation"],
      ["Enter", "Open conversation · reply all inside it"],
      ["Esc", "Back to the list · close"],
      ["Tab / ⇧Tab", "Next / previous split · cycle links in a conversation"],
      ["N / P", "Next / previous message"],
      ["O", "Expand or collapse message"],
      ["⇧O", "Expand everything"],
      ["X", "Select · ⇧J / ⇧K extend"],
      ["/", "Search"],
      ["⌘K", "Commands"],
      ["⌃1…9", "Switch account"],
    ],
  ],
  [
    "Act",
    [
      ["E", "Mark done"],
      ["⇧E", "Return to inbox"],
      ["H", "Snooze"],
      ["S", "Star"],
      ["U", "Read / unread"],
      ["#", "Trash"],
      ["!", "Spam"],
      ["L", "Labels"],
      ["Y", "Remove current label"],
      ["Z", "Undo"],
    ],
  ],
  [
    "Write",
    [
      ["C", "Compose"],
      ["R", "Reply"],
      ["F", "Forward"],
      ["⌘Enter", "Send"],
      ["⌘⇧O / C / B", "To / Cc / Bcc"],
      ["⌘⇧F", "From"],
      ["⌘⇧S / M", "Subject / message"],
      ["⌘⇧A", "Attach"],
      ["⌘⇧,", "Discard draft"],
    ],
  ],
  [
    "Go to",
    [
      ["G I / G O", "Important / Other"],
      ["G E", "Done"],
      ["G H", "Snoozed"],
      ["G S", "Starred"],
      ["G T", "Sent"],
      ["G D", "Drafts"],
      ["G L", "Label"],
      ["G # / G !", "Trash / Spam"],
    ],
  ],
];

const closeSheet = () => useChrome.getState().set({ sheet: null });

function Sheet({
  title,
  children,
  wide,
}: {
  title: string;
  children: React.ReactNode;
  wide?: boolean;
}) {
  return (
    // The backdrop closes on a click outside the sheet; Esc closes from the keyboard.
    <div
      role="presentation"
      className="fixed inset-0 z-30 bg-black/10"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) closeSheet();
      }}
    >
      <dialog
        open
        aria-modal="true"
        aria-label={title}
        className={`fade-in absolute top-16 left-1/2 m-0 max-h-[80vh] -translate-x-1/2 overflow-y-auto rounded-sheet border-0 bg-bg px-8 py-5 text-text shadow-float ${wide ? "w-[760px]" : "w-[550px]"}`}
      >
        <div className="mb-4 text-[15px] font-semibold">{title}</div>
        {children}
      </dialog>
    </div>
  );
}

function Shortcuts() {
  return (
    <Sheet title="Keyboard shortcuts" wide>
      <div className="grid grid-cols-2 gap-x-8 gap-y-5">
        {SHORTCUTS.map(([group, rows]) => (
          <div key={group}>
            <div className="mb-1 text-hint font-medium text-text-3 uppercase">{group}</div>
            {rows.map(([k, d]) => (
              <div
                key={k}
                className="flex h-7 items-center justify-between border-b border-border-2 text-row"
              >
                <span className="text-text-2">{d}</span>
                <kbd className="rounded-[4px] bg-fill px-1.5 text-hint font-medium">{k}</kbd>
              </div>
            ))}
          </div>
        ))}
      </div>
    </Sheet>
  );
}

function Toggle({
  on,
  onChange,
  label,
}: {
  on: boolean;
  onChange: (on: boolean) => void;
  label: string;
}) {
  return (
    <button
      role="switch"
      aria-checked={on}
      aria-label={label}
      onClick={() => onChange(!on)}
      className={`relative h-5 w-9 rounded-full transition-colors ${on ? "bg-accent" : "bg-border"}`}
    >
      <span
        className={`absolute top-0.5 h-4 w-4 rounded-full bg-white transition-all ${on ? "left-[18px]" : "left-0.5"}`}
      />
    </button>
  );
}

function AccountBlock({ a }: { a: AccountInfo }) {
  const summary = useSummary(a.id);
  const [confirm, setConfirm] = useState(false);
  const queued = summary?.queuedOps ?? 0;
  const sends = summary?.outbox.filter((o) => o.state !== "sent").length ?? 0;
  return (
    <div className="border-t border-border-2 py-3">
      <div className="mb-2 text-row font-semibold">{a.email}</div>
      <div className="flex items-center justify-between py-1 text-row">
        <span>Mute notifications</span>
        <Toggle
          label="Mute notifications"
          on={a.muted}
          onChange={(on) =>
            act({ type: "setSetting", account: a.id, key: "muted", value: on ? "on" : "off" })
          }
        />
      </div>
      <div className="flex flex-wrap gap-2 pt-2">
        <button
          className="rounded-control border border-border px-2 py-1 text-row hover:bg-fill-half"
          onClick={() => {
            useChrome.getState().setAccount(a.id);
            useChrome.getState().set({ sheet: "splits" });
          }}
        >
          Split Inbox settings
        </button>
        {a.needsSignin && (
          <button
            className="rounded-control bg-accent px-2 py-1 text-row text-white"
            onClick={() => void signInAgain(a.id)}
          >
            Sign in again
          </button>
        )}
        {!confirm ? (
          <button
            className="rounded-control px-2 py-1 text-row text-danger hover:bg-fill-half"
            onClick={() => setConfirm(true)}
          >
            Remove account
          </button>
        ) : (
          <span className="flex items-center gap-2 text-row">
            <span className="text-danger">
              Remove {a.email}?
              {(queued > 0 || sends > 0) &&
                ` ${queued} queued actions and ${sends} sends will be lost.`}
            </span>
            <button
              className="rounded-control bg-danger px-2 py-1 text-white"
              onClick={() => act({ type: "removeAccount", account: a.id })}
            >
              Remove
            </button>
            <button className="px-2 py-1 text-text-2" onClick={() => setConfirm(false)}>
              Cancel
            </button>
          </span>
        )}
      </div>
    </div>
  );
}

function Settings() {
  const app = useAppState();
  if (!app) return null;
  return (
    <Sheet title="Settings">
      <div className="flex items-center justify-between py-2 text-row">
        <span>Load remote images</span>
        <Toggle
          label="Load remote images"
          on={app.settings.remoteImages}
          onChange={(on) =>
            act({
              type: "setSetting",
              account: null,
              key: "remote_images",
              value: on ? "on" : "off",
            })
          }
        />
      </div>
      {app.accounts.map((a) => (
        <AccountBlock key={a.id} a={a} />
      ))}
    </Sheet>
  );
}

const STARTERS: [string, string][] = [
  ["Newsletters", "is:list"],
  ["Promotions", "label:CATEGORY_PROMOTIONS"],
  ["Social", "label:CATEGORY_SOCIAL"],
  ["Updates", "label:CATEGORY_UPDATES"],
];

function Splits() {
  const { account } = useCurrent();
  const summary = useSummary(account);
  // Unedited, the sheet shows the live splits; the first edit takes a copy.
  const [draft, setRows] = useState<SplitInfo[] | null>(null);
  const [deleted, setDeleted] = useState<number[]>([]);
  // New splits get negative ids until saved.
  const nextNewId = useRef(-1);
  if (!summary || account === null) return null;
  const rows = draft ?? summary.splits;
  const update = (i: number, patch: Partial<SplitInfo>) =>
    setRows(rows.map((r, j) => (i === j ? { ...r, ...patch } : r)));
  const move = (i: number, d: number) => {
    const j = i + d;
    const a = rows[i];
    const b = rows[j];
    // Only custom splits reorder, among themselves.
    if (!a || !b || a.kind !== "custom" || b.kind !== "custom") return;
    const next = [...rows];
    next[i] = b;
    next[j] = a;
    setRows(next);
  };
  const add = (name: string, query: string) => {
    const id = nextNewId.current--;
    setRows((current) => {
      const r = current ?? summary.splits;
      // Custom splits come first, then Important, then Other.
      const fixed = r.findIndex((x) => x.kind !== "custom");
      const s: SplitInfo = {
        id,
        name,
        kind: "custom",
        query,
        enabled: true,
        hideWhenEmpty: true,
        position: 0,
      };
      return fixed < 0 ? [...r, s] : [...r.slice(0, fixed), s, ...r.slice(fixed)];
    });
  };
  return (
    <Sheet title={`Split Inbox · ${summary.account.email}`}>
      {rows.map((s, i) => (
        <div key={s.id} className="flex items-center gap-2 border-b border-border-2 py-2 text-row">
          <div className="flex flex-col">
            <button className="text-hint text-text-3" onClick={() => move(i, -1)}>
              ▲
            </button>
            <button className="text-hint text-text-3" onClick={() => move(i, 1)}>
              ▼
            </button>
          </div>
          <input
            className="w-28 rounded-control border border-border px-1.5 py-0.5"
            value={s.name}
            onChange={(e) => update(i, { name: e.target.value })}
          />
          {s.kind === "custom" ? (
            <input
              className="flex-1 rounded-control border border-border px-1.5 py-0.5 font-mono text-[12px]"
              value={s.query}
              placeholder="from:boss@acme.com OR label:Receipts"
              onChange={(e) => update(i, { query: e.target.value })}
            />
          ) : (
            <span className="flex-1 text-text-3">
              {s.kind === "important" ? "Person-to-person mail" : "Everything else"}
            </span>
          )}
          <label className="flex items-center gap-1 text-hint text-text-2" title="Hide while empty">
            <input
              type="checkbox"
              checked={s.hideWhenEmpty}
              onChange={(e) => update(i, { hideWhenEmpty: e.target.checked })}
            />
            hide empty
          </label>
          {s.kind !== "other" && (
            <Toggle
              label={`Turn ${s.name} on or off`}
              on={s.enabled}
              onChange={(on) => update(i, { enabled: on })}
            />
          )}
          {s.kind === "custom" && (
            <button
              className="text-danger"
              onClick={() => {
                if (s.id > 0) setDeleted([...deleted, s.id]);
                setRows(rows.filter((_, j) => j !== i));
              }}
            >
              ×
            </button>
          )}
        </div>
      ))}
      <div className="mt-3 flex flex-wrap gap-2">
        <button
          className="rounded-control border border-border px-2 py-1 text-row"
          onClick={() => add("New split", "")}
        >
          Add split
        </button>
        {STARTERS.filter(([n]) => !rows.some((r) => r.name === n)).map(([n, q]) => (
          <button
            key={n}
            className="rounded-control border border-border px-2 py-1 text-row text-text-2"
            onClick={() => add(n, q)}
          >
            + {n}
          </button>
        ))}
        <div className="flex-1" />
        <button
          className="rounded-control bg-accent px-3 py-1 text-row text-white"
          onClick={() => {
            act({ type: "saveSplits", account, splits: rows, deleted });
            useChrome.getState().set({ sheet: null });
          }}
        >
          Save
        </button>
      </div>
    </Sheet>
  );
}

export function Sheets() {
  const sheet = useChrome((s) => s.sheet);
  if (sheet === "shortcuts") return <Shortcuts />;
  if (sheet === "settings") return <Settings />;
  if (sheet === "splits") return <Splits />;
  return null;
}
