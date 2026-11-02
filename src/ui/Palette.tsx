import { Command } from "cmdk";
import { useEffect, useMemo, useState } from "react";

import { switchAccount } from "../App";
import {
  applyLabel,
  goTo,
  markDone,
  removeViewLabel,
  returnToInbox,
  rowOf,
  snooze,
  spam,
  targets,
  toggleRead,
  toggleStar,
  trash,
  undo,
} from "../commands";
import { startCompose, startReply } from "../compose/start";
import { act, addAccount, snoozeOptions } from "../ipc/bridge";
import type { PinKind, View } from "../ipc/types";
import { appHooks } from "../keyboard/keys";
import { goSplit } from "../keyboard/nav";
import { measure } from "../keyboard/meter";
import { useChrome, type PaletteMode } from "../store/chrome";
import { useAppState, useCurrent, useSummary } from "../store/hooks";
import { draftPaletteItems } from "./ComposeCommands";

type Item = { id: string; label: string; keys?: string; run: () => void; keep?: boolean };

const close = () => useChrome.getState().set({ palette: null });
const view = (v: View) => () => goTo({ kind: "view", view: v });

function Chip({ keys }: { keys: string }) {
  return (
    <span className="flex gap-1">
      {keys.split(" ").map((k, i) => (
        <kbd
          key={i}
          className="rounded-[4px] bg-[#3a3835] px-1.5 text-hint font-medium text-palette-text-2"
        >
          {k}
        </kbd>
      ))}
    </span>
  );
}

function useCommands(mode: PaletteMode, query: string): { items: Item[]; placeholder: string } {
  const { account } = useCurrent();
  const summary = useSummary(account);
  const app = useAppState();
  const [snoozes, setSnoozes] = useState<{ label: string; at: number }[]>([]);

  useEffect(() => {
    if (mode.kind !== "snooze") return;
    let live = true;
    void snoozeOptions(query).then((o) => live && setSnoozes(o));
    return () => {
      live = false;
    };
  }, [mode.kind, query]);

  return useMemo(() => {
    const userLabels = (summary?.labels ?? []).filter((l) => !l.system);
    const muted = summary?.account.muted ?? false;
    switch (mode.kind) {
      case "commands": {
        if (mode.draftActions) return { items: draftPaletteItems(), placeholder: "Draft actions" };
        const items: Item[] = [
          { id: "done", label: "Mark done", keys: "E", run: markDone },
          { id: "inbox", label: "Return to inbox", keys: "⇧ E", run: returnToInbox },
          {
            id: "snooze",
            label: "Snooze",
            keys: "H",
            run: () => appHooks.openPalette({ kind: "snooze" }),
            keep: true,
          },
          { id: "trash", label: "Trash", keys: "#", run: trash },
          { id: "spam", label: "Mark as spam", keys: "!", run: spam },
          { id: "star", label: "Star or unstar", keys: "S", run: toggleStar },
          { id: "read", label: "Mark read or unread", keys: "U", run: toggleRead },
          {
            id: "label",
            label: "Add or remove label",
            keys: "L",
            run: () => appHooks.openPalette({ kind: "labels" }),
            keep: true,
          },
          { id: "unlabel", label: "Remove current label", keys: "Y", run: removeViewLabel },
          { id: "undo", label: "Undo", keys: "Z", run: undo },
          { id: "reply", label: "Reply", keys: "R", run: () => startReply("reply") },
          { id: "replyall", label: "Reply all", keys: "↵", run: () => startReply("replyAll") },
          { id: "forward", label: "Forward", keys: "F", run: () => startReply("forward") },
          { id: "compose", label: "Compose", keys: "C", run: startCompose },
          { id: "search", label: "Search", keys: "/", run: () => appHooks.focusSearch() },
          { id: "g-inbox", label: "Go to Inbox", run: view({ kind: "inbox" }) },
          {
            id: "g-imp",
            label: "Go to Important",
            keys: "G I",
            run: () => goSplit("important", appHooks.splitIdOf),
          },
          {
            id: "g-other",
            label: "Go to Other",
            keys: "G O",
            run: () => goSplit("other", appHooks.splitIdOf),
          },
          { id: "g-done", label: "Go to Done", keys: "G E", run: view({ kind: "done" }) },
          { id: "g-snoozed", label: "Go to Snoozed", keys: "G H", run: view({ kind: "snoozed" }) },
          { id: "g-starred", label: "Go to Starred", keys: "G S", run: view({ kind: "starred" }) },
          { id: "g-sent", label: "Go to Sent", keys: "G T", run: view({ kind: "sent" }) },
          { id: "g-drafts", label: "Go to Drafts", keys: "G D", run: view({ kind: "drafts" }) },
          { id: "g-trash", label: "Go to Trash", keys: "G #", run: view({ kind: "trash" }) },
          { id: "g-spam", label: "Go to Spam", keys: "G !", run: view({ kind: "spam" }) },
          {
            id: "g-label",
            label: "Go to label",
            keys: "G L",
            run: () => appHooks.openPalette({ kind: "goLabel" }),
            keep: true,
          },
          {
            id: "split-add",
            label: "Add to split",
            run: () => appHooks.openPalette({ kind: "addToSplit" }),
            keep: true,
          },
          {
            id: "split-imp",
            label: "Move to Important",
            run: () => appHooks.openPalette({ kind: "addToSplit", preset: "important" }),
            keep: true,
          },
          {
            id: "split-other",
            label: "Move to Other",
            run: () => appHooks.openPalette({ kind: "addToSplit", preset: "other" }),
            keep: true,
          },
          {
            id: "split-settings",
            label: "Split Inbox settings",
            run: () => useChrome.getState().set({ sheet: "splits" }),
          },
          {
            id: "switch",
            label: "Switch account",
            run: () => appHooks.openPalette({ kind: "switchAccount" }),
            keep: true,
          },
          { id: "add-account", label: "Add account", run: () => void addAccount() },
          {
            id: "mute",
            label: muted ? "Unmute notifications" : "Mute notifications",
            run: () =>
              act({ type: "setSetting", account, key: "muted", value: muted ? "off" : "on" }),
          },
          {
            id: "settings",
            label: "Settings",
            keys: "⌘ ,",
            run: () => useChrome.getState().set({ sheet: "settings" }),
          },
          {
            id: "shortcuts",
            label: "Keyboard shortcuts",
            keys: "?",
            run: () => useChrome.getState().set({ sheet: "shortcuts" }),
          },
        ];
        return { items, placeholder: "Type a command" };
      }
      case "labels": {
        const t = targets();
        const items: Item[] = userLabels.map((l) => {
          const all = t.length > 0 && t.every((id) => rowOf(String(id))?.labels.includes(l.id));
          return { id: l.id, label: `${all ? "✓ " : ""}${l.name}`, run: () => applyLabel(l.id) };
        });
        const q = query.trim();
        if (
          q &&
          !userLabels.some((l) => l.name.toLowerCase() === q.toLowerCase()) &&
          account !== null
        ) {
          items.push({
            id: "create",
            label: `Create label "${q}"`,
            run: () => {
              const id = `local-${crypto.randomUUID()}`;
              act({
                type: "createLabel",
                account,
                id,
                name: q,
                apply: {
                  account,
                  kind: "label",
                  threads: targets(),
                  add: [id],
                  remove: [],
                  snooze: null,
                  undoable: true,
                },
              });
            },
          });
        }
        return { items, placeholder: "Label as…" };
      }
      case "goLabel":
        return {
          items: userLabels.map((l) => ({
            id: l.id,
            label: l.name,
            run: view({ kind: "label", label: l.id }),
          })),
          placeholder: "Go to label",
        };
      case "snooze":
        return {
          items: snoozes.map((o) => ({ id: `${o.at}`, label: o.label, run: () => snooze(o.at) })),
          placeholder: "Snooze until… (try “fri 9am”)",
        };
      case "switchAccount":
        return {
          items: (app?.accounts ?? []).map((a, i) => ({
            id: String(a.id),
            label: a.email,
            keys: `⌃ ${i + 1}`,
            run: () => switchAccount(a.id),
          })),
          placeholder: "Switch account",
        };
      case "addToSplit": {
        const t = targets()[0];
        const row = t !== undefined ? rowOf(String(t)) : undefined;
        if (!row || account === null) return { items: [], placeholder: "Nothing selected" };
        const email = row.senderEmail.toLowerCase();
        const domain = email.split("@")[1] ?? "";
        const rowLabels = userLabels.filter((l) => row.labels.includes(l.id));
        const choices: { kind: PinKind; value: string; label: string }[] = [
          { kind: "sender", value: email, label: `Sender ${email}` },
          ...(domain
            ? [{ kind: "domain" as PinKind, value: domain, label: `Domain ${domain}` }]
            : []),
          ...rowLabels.map((l) => ({
            kind: "label" as PinKind,
            value: l.name,
            label: `Label ${l.name}`,
          })),
        ];
        const splits = (summary?.splits ?? []).filter((s) =>
          mode.preset ? s.kind === mode.preset : s.kind !== "other" || true,
        );
        const items: Item[] = [];
        for (const c of choices) {
          for (const s of splits) {
            items.push({
              id: `${c.kind}:${c.value}:${s.id}`,
              label: mode.preset ? c.label : `${c.label} → ${s.name}`,
              run: () => act({ type: "pin", account, kind: c.kind, value: c.value, split: s.id }),
            });
          }
        }
        return {
          items,
          placeholder:
            mode.preset === "important"
              ? "Move to Important"
              : mode.preset === "other"
                ? "Move to Other"
                : "Add to split",
        };
      }
      case "from":
        return { items: draftPaletteItems(mode.draft), placeholder: "Send from" };
    }
  }, [mode, query, summary, app, account, snoozes]);
}

export function Palette() {
  const mode = useChrome((s) => s.palette);
  if (!mode) return null;
  return <PaletteBody key={JSON.stringify(mode)} mode={mode} />;
}

const itemValue = (it: Item) => `${it.label} ${it.id}`;

function PaletteBody({ mode }: { mode: PaletteMode }) {
  const [query, setQuery] = useState("");
  const { items, placeholder } = useCommands(mode, query);
  const filtered = mode.kind === "snooze";
  // Snooze options arrive from Rust after each keystroke: keep the first one
  // selected so Enter always picks something.
  const [selected, setSelected] = useState("");
  const firstValue = items[0] ? itemValue(items[0]) : "";
  useEffect(() => {
    if (filtered) setSelected(firstValue);
  }, [filtered, firstValue]);
  return (
    <div className="fixed inset-0 z-40" onMouseDown={close}>
      <div
        className="fade-in absolute left-1/2 top-[33%] w-[680px] -translate-x-1/2 -translate-y-1/3 overflow-hidden rounded-sheet bg-palette-bg text-palette-text shadow-float"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <Command
          shouldFilter={!filtered}
          value={filtered ? selected : undefined}
          onValueChange={filtered ? setSelected : undefined}
          loop
          onKeyDown={(e) => {
            const start = performance.now();
            if (e.key === "Escape") {
              e.preventDefault();
              e.stopPropagation();
              close();
            } else measure("paletteKey", start);
          }}
        >
          <Command.Input
            autoFocus
            value={query}
            onValueChange={setQuery}
            placeholder={placeholder}
            className="h-12 w-full border-b border-[#33312e] bg-transparent px-4 text-[15px] text-palette-text outline-none placeholder:text-palette-text-2"
          />
          <Command.List className="max-h-[288px] overflow-y-auto py-1">
            <Command.Empty className="px-4 py-2 text-row text-palette-text-2">
              No results
            </Command.Empty>
            {items.map((it) => (
              <Command.Item
                key={it.id}
                value={itemValue(it)}
                onSelect={() => {
                  const start = performance.now();
                  if (!it.keep) close();
                  it.run();
                  measure(
                    mode.kind === "snooze"
                      ? "snoozeChoice"
                      : mode.kind === "labels"
                        ? "labelChoice"
                        : "view",
                    start,
                  );
                }}
                className="mx-1 flex h-row cursor-default items-center justify-between rounded-control px-3 text-row"
              >
                <span className="truncate">{it.label}</span>
                {it.keys && <Chip keys={it.keys} />}
              </Command.Item>
            ))}
          </Command.List>
        </Command>
      </div>
    </div>
  );
}
