import { useRef, useState } from "react";
import { toast } from "sonner";

import { act, signInAgain } from "../ipc/bridge";
import { keyText, sheet as shortcutSheet } from "../keyboard/table";
import type { AccountInfo, SplitInfo } from "../ipc/types";
import { closeSheet, openSplitSettings } from "../navigation";
import { useChrome } from "../store/chrome";
import { reportSignIn } from "./signIn";
import { useAppState, useCurrent, useSummary } from "../store/hooks";

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
      className="fixed inset-0 z-30 bg-scrim"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) closeSheet();
      }}
    >
      <dialog
        open
        aria-modal="true"
        aria-label={title}
        className={`fade-in absolute top-16 left-1/2 m-0 max-h-sheet-max -translate-x-1/2 overflow-y-auto rounded-lg border-0 bg-bg px-8 py-5 text-text shadow-float ${wide ? "w-sheet-wide" : "w-sheet"}`}
      >
        <div className="mb-4 text-title font-semibold">{title}</div>
        {children}
      </dialog>
    </div>
  );
}

function Shortcuts() {
  return (
    <Sheet title="Keyboard shortcuts" wide>
      <div className="grid grid-cols-2 gap-x-8 gap-y-5">
        {shortcutSheet().map(({ group, commands }) => (
          <div key={group}>
            <div className="mb-1 text-hint font-medium text-text-3 uppercase">{group}</div>
            {commands.map((cmd) => (
              <div
                key={cmd.id}
                className="flex min-h-7 items-center justify-between gap-3 border-b border-border-2 py-1 text-row"
              >
                <span className="text-text-2">{cmd.label}</span>
                <kbd className="shrink-0 rounded-sm bg-fill px-1.5 text-hint font-medium text-text-2">
                  {keyText(cmd)}
                </kbd>
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
        className={`absolute top-0.5 h-4 w-4 rounded-full bg-bg transition-all ${on ? "left-4.5" : "left-0.5"}`}
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
          onChange={(on) => act({ type: "setSetting", setting: "muted", account: a.id, on })}
        />
      </div>
      <div className="flex flex-wrap gap-2 pt-2">
        <button
          className="rounded-md border border-border px-2 py-1 text-row hover:bg-fill-half"
          onClick={() => openSplitSettings(a.id)}
        >
          Split Inbox settings
        </button>
        {a.needsSignin && (
          <button
            className="rounded-md bg-accent px-2 py-1 text-row text-inverse"
            onClick={() => reportSignIn(signInAgain(a.id))}
          >
            Sign in again
          </button>
        )}
        {!confirm ? (
          <button
            className="rounded-md px-2 py-1 text-row text-danger hover:bg-fill-half"
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
              className="rounded-md bg-danger px-2 py-1 text-inverse"
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
          onChange={(on) => act({ type: "setSetting", setting: "remoteImages", on })}
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
  const [edited, setEdited] = useState<SplitInfo[] | null>(null);
  const [deleted, setDeleted] = useState<number[]>([]);
  // Why the core refused a rule, by split id, at the last Save.
  const [refused, setRefused] = useState<Record<number, string>>({});
  const [saving, setSaving] = useState(false);
  // New splits get negative ids until saved.
  const nextNewId = useRef(-1);
  if (!summary || account === null) return null;
  const rows = edited ?? summary.splits;
  const update = (i: number, patch: Partial<SplitInfo>) => {
    const id = rows[i]?.id;
    // Editing a refused rule clears its message until the next Save.
    if (patch.query !== undefined && id !== undefined && id in refused) {
      setRefused(Object.fromEntries(Object.entries(refused).filter(([k]) => k !== String(id))));
    }
    setEdited(rows.map((r, j) => (i === j ? { ...r, ...patch } : r)));
  };
  // The sheet closes once the core has saved; a refused rule keeps the
  // sheet and every edit on screen (SPEC Split Inbox). One save at a time:
  // a second would add the new splits again.
  const save = () => {
    if (saving) return;
    setSaving(true);
    act({ type: "saveSplits", account, splits: rows, deleted }, ({ refusal }) => {
      setSaving(false);
      if (refusal === null) {
        if (useChrome.getState().sheet === "splits") closeSheet();
      } else if (refusal.split === null) {
        toast.error(refusal.message);
      } else {
        setRefused({ [refusal.split]: refusal.message });
      }
    });
  };
  const move = (i: number, d: number) => {
    const j = i + d;
    const a = rows[i];
    const b = rows[j];
    // Only custom splits reorder, among themselves.
    if (!a || !b || a.kind !== "custom" || b.kind !== "custom") return;
    const next = [...rows];
    next[i] = b;
    next[j] = a;
    setEdited(next);
  };
  const add = (name: string, query: string) => {
    const id = nextNewId.current--;
    setEdited((current) => {
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
        <div key={s.id} className="border-b border-border-2 py-2 text-row">
          <div className="flex items-center gap-2">
            <div className="flex flex-col">
              <button className="text-hint text-text-3" onClick={() => move(i, -1)}>
                ▲
              </button>
              <button className="text-hint text-text-3" onClick={() => move(i, 1)}>
                ▼
              </button>
            </div>
            <input
              className="w-28 rounded-md border border-border px-1.5 py-0.5"
              value={s.name}
              onChange={(e) => update(i, { name: e.target.value })}
            />
            {s.kind === "custom" ? (
              <input
                className="flex-1 rounded-md border border-border px-1.5 py-0.5 font-mono text-code"
                value={s.query}
                placeholder="from:boss@acme.com OR label:Receipts"
                aria-invalid={s.id in refused || undefined}
                aria-describedby={s.id in refused ? `split-rule-${s.id}` : undefined}
                onChange={(e) => update(i, { query: e.target.value })}
              />
            ) : (
              <span className="flex-1 text-text-3">
                {s.kind === "important" ? "Person-to-person mail" : "Everything else"}
              </span>
            )}
            <label
              className="flex items-center gap-1 text-hint text-text-2"
              title="Hide while empty"
            >
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
                  setEdited(rows.filter((_, j) => j !== i));
                }}
              >
                ×
              </button>
            )}
          </div>
          {refused[s.id] !== undefined && (
            <p id={`split-rule-${s.id}`} role="alert" className="mt-1 text-hint text-danger">
              {refused[s.id]}
            </p>
          )}
        </div>
      ))}
      <div className="mt-3 flex flex-wrap gap-2">
        <button
          className="rounded-md border border-border px-2 py-1 text-row"
          onClick={() => add("New split", "")}
        >
          Add split
        </button>
        {STARTERS.filter(([n]) => !rows.some((r) => r.name === n)).map(([n, q]) => (
          <button
            key={n}
            className="rounded-md border border-border px-2 py-1 text-row text-text-2"
            onClick={() => add(n, q)}
          >
            + {n}
          </button>
        ))}
        <div className="flex-1" />
        <button className="rounded-md bg-accent px-3 py-1 text-row text-inverse" onClick={save}>
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
