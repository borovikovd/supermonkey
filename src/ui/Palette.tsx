import { Command } from "cmdk";
import { useEffect, useState } from "react";
import { toast } from "sonner";

import { stillShown, withFixedTargets } from "../acting";
import { applyLabel, block, snooze, spam, targets, unsubscribe } from "../commands";
import { formatAddr } from "../compose/RecipientField";
import { mountedComposer } from "../compose/session";
import { act, blockedSenders, signInAgain, snoozeOptions, unblockSender } from "../ipc/bridge";
import type { BlockedSenders, PinKind, SnoozeChoice } from "../ipc/types";
import { registerKeys } from "../keyboard/dispatch";
import { measure } from "../keyboard/meter";
import {
  accountChips,
  context,
  keyChips,
  labelOf,
  paletteCommands,
  runFromPalette,
} from "../keyboard/table";
import { closePalette, goTo, switchAccount } from "../navigation";
import { useChrome, type PaletteMode } from "../store/chrome";
import { rowOf } from "../store/current";
import { useAppState, useCurrent, useSummary } from "../store/hooks";
import { reportSignIn } from "./signIn";

type Item = { id: string; label: string; keys?: string[]; run: () => void };

const close = closePalette;

function Chip({ keys }: { keys: string[] }) {
  return (
    <span className="flex gap-1">
      {keys.map((k) => (
        <kbd
          key={k}
          className="rounded-sm bg-palette-key px-1.5 text-hint font-medium text-palette-text-2"
        >
          {k}
        </kbd>
      ))}
    </span>
  );
}

type Commands = {
  items: Item[];
  placeholder: string;
  /** Snooze: the options on screen are for an earlier query. */
  stale?: boolean;
  /** Snooze: snoozes what the palette opened on until `at`. */
  snoozeAt?: (at: number) => void;
};

function useCommands(mode: PaletteMode, query: string): Commands {
  const { account } = useCurrent();
  const summary = useSummary(account);
  const app = useAppState();
  const [snoozes, setSnoozes] = useState<{ query: string; options: SnoozeChoice[] } | null>(null);
  const [blocked, setBlocked] = useState<
    BlockedSenders | { kind: "error"; message: string } | null
  >(null);
  // What the palette acts on, read once when it opens: keys go to the palette
  // while it is open, so the selection cannot move under it. Opened from a
  // palette, it is bound to what that one opened on; opened by a toast about
  // one conversation, to that conversation, on its own account.
  const [opened] = useState(() => {
    const threads =
      mode.kind === "block" && mode.of
        ? mode.of.account === useChrome.getState().account
          ? [mode.of.thread]
          : []
        : mode.threads
          ? stillShown(mode.threads)
          : targets();
    return { threads, rows: threads.map((id) => rowOf(String(id))), ctx: context() };
  });

  useEffect(() => {
    if (mode.kind !== "snooze") return;
    let live = true;
    void snoozeOptions(query).then((options) => live && setSnoozes({ query, options }));
    return () => {
      live = false;
    };
  }, [mode.kind, query]);

  useEffect(() => {
    if (mode.kind !== "blocked" || account === null) return;
    let live = true;
    blockedSenders(account)
      .then((b) => live && setBlocked(b))
      .catch((err: unknown) => live && setBlocked({ kind: "error", message: String(err) }));
    return () => {
      live = false;
    };
  }, [mode.kind, account]);

  const userLabels = (summary?.labels ?? []).filter((l) => !l.system);
  switch (mode.kind) {
    case "commands": {
      // The commands that work where the palette opened; while a composer is
      // on screen, its draft actions (SPEC Keyboard).
      const composing = opened.ctx.composer !== null;
      return {
        items: paletteCommands(opened.ctx).map((cmd) => ({
          id: cmd.id,
          label: labelOf(cmd, opened.ctx),
          keys: keyChips(cmd),
          run: () => withFixedTargets(opened.threads, () => runFromPalette(cmd)),
        })),
        placeholder: composing ? "Draft actions" : "Type a command",
      };
    }
    case "labels": {
      const items: Item[] = userLabels.map((l) => {
        const all =
          opened.rows.length > 0 && opened.rows.every((row) => row?.labels.includes(l.id));
        return {
          id: l.id,
          label: `${all ? "✓ " : ""}${l.name}`,
          run: () => applyLabel(l.id, opened.threads),
        };
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
                threads: opened.threads,
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
          run: () => goTo({ kind: "label", label: l.id }),
        })),
        placeholder: "Go to label",
      };
    case "snooze":
      return {
        items: (snoozes?.options ?? []).map((o) => ({
          id: `${o.at}`,
          label: o.label,
          run: () => snooze(o.at, opened.threads),
        })),
        placeholder: "Snooze until… (try “fri 9am”)",
        stale: snoozes?.query !== query,
        snoozeAt: (at) => snooze(at, opened.threads),
      };
    case "unsubscribe":
      return {
        items: [
          { id: "keep", label: "Unsubscribe", run: () => void unsubscribe("keep", opened.threads) },
          {
            id: "done",
            label: "Unsubscribe, and mark done all",
            run: () => void unsubscribe("done", opened.threads),
          },
          {
            id: "trash",
            label: "Unsubscribe, and trash all",
            run: () => void unsubscribe("trash", opened.threads),
          },
        ],
        placeholder: "Unsubscribe from this sender",
      };
    case "block":
    case "spam": {
      const thread = opened.threads[0];
      if (thread === undefined) return { items: [], placeholder: "Nothing selected" };
      const email = opened.rows[0]?.senderEmail.toLowerCase() ?? "";
      const domain = email.split("@")[1] ?? "";
      const asSpam = mode.kind === "spam";
      const then = asSpam ? "Mark spam, and block" : "Block";
      return {
        items: [
          ...(asSpam ? [{ id: "spam", label: "Mark spam", run: () => spam(opened.threads) }] : []),
          {
            id: "address",
            label: email ? `${then} ${email}` : `${then} the sender`,
            run: () => void block(thread, false, asSpam),
          },
          ...(domain
            ? [
                {
                  id: "domain",
                  label: `${then} everyone at ${domain}`,
                  run: () => void block(thread, true, asSpam),
                },
              ]
            : []),
        ],
        placeholder: asSpam ? "Mark as spam" : "Block",
      };
    }
    case "blocked": {
      if (blocked === null) return { items: [], placeholder: "Loading blocked senders…" };
      if (blocked.kind === "error") return { items: [], placeholder: blocked.message };
      if (blocked.kind === "needsPermission") {
        return {
          items: [
            {
              id: "signin",
              label: "Sign in again to manage blocked senders",
              run: () => account !== null && reportSignIn(signInAgain(account)),
            },
          ],
          placeholder: "Blocked senders",
        };
      }
      return {
        items: blocked.senders.map((b) => ({
          id: b.id,
          label: `Unblock ${b.from}`,
          run: () => {
            if (account === null) return;
            unblockSender(account, b.id)
              .then(() => toast(`Unblocked ${b.from}`))
              .catch((err: unknown) => toast.error(`Couldn't unblock: ${String(err)}`));
          },
        })),
        placeholder: blocked.senders.length > 0 ? "Blocked senders" : "No blocked senders",
      };
    }
    case "switchAccount":
      return {
        items: (app?.accounts ?? []).map((a, i) => ({
          id: String(a.id),
          label: a.email,
          keys: accountChips(i + 1),
          run: () => switchAccount(a.id),
        })),
        placeholder: "Switch account",
      };
    case "addToSplit": {
      const row = opened.rows[0];
      if (!row || account === null) return { items: [], placeholder: "Nothing selected" };
      const email = row.senderEmail.toLowerCase();
      const domain = email.split("@")[1] ?? "";
      const rowLabels = userLabels.filter((l) => row.labels.includes(l.id));
      const choices: { kind: PinKind; value: string; label: string }[] = [
        { kind: "sender", value: email, label: `Sender ${email}` },
        ...(domain ? [{ kind: "domain" as const, value: domain, label: `Domain ${domain}` }] : []),
        ...rowLabels.map((l) => ({
          kind: "label" as const,
          value: l.name,
          label: `Label ${l.name}`,
        })),
      ];
      const splits = (summary?.splits ?? []).filter((s) => !mode.preset || s.kind === mode.preset);
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
    case "from": {
      const sendAs = app?.accounts.find((a) => a.id === account)?.sendAs ?? [];
      return {
        items: sendAs.map((s) => ({
          id: s.email,
          label: formatAddr(s),
          run: () => mountedComposer()?.setFrom({ name: s.name || null, email: s.email }),
        })),
        placeholder: "Send from",
      };
    }
  }
}

export function Palette() {
  const mode = useChrome((s) => s.palette);
  if (!mode) return null;
  return <PaletteBody key={JSON.stringify(mode)} mode={mode} />;
}

const itemValue = (it: Item) => `${it.label} ${it.id}`;

function PaletteBody({ mode }: { mode: PaletteMode }) {
  const [query, setQuery] = useState("");
  const { items, placeholder, stale, snoozeAt } = useCommands(mode, query);
  // Snooze options arrive from Rust, already filtered, after each keystroke:
  // keep the first one selected so Enter always picks something.
  const fromRust = mode.kind === "snooze";
  const [picked, setPicked] = useState("");
  const firstValue = items[0] ? itemValue(items[0]) : "";

  // Keys reach the palette first (src/keyboard/dispatch.ts); the rest go to cmdk.
  useEffect(
    () =>
      registerKeys("palette", (e) => {
        const start = performance.now();
        if (e.key === "Escape") {
          e.preventDefault();
          close();
          return true;
        }
        if (e.key === "Enter" && stale) {
          // The options on screen are for an earlier query: Enter takes the
          // first option for what is typed now, once it arrives.
          e.preventDefault();
          void snoozeOptions(query).then(([first]) => {
            if (!first || useChrome.getState().palette !== mode) return;
            close();
            snoozeAt?.(first.at);
          });
          return true;
        }
        measure("paletteKey", start);
        return false;
      }),
    [stale, query, mode, snoozeAt],
  );
  const selected = items.some((it) => itemValue(it) === picked) ? picked : firstValue;
  return (
    // The backdrop closes on a click outside the palette; Esc closes from the keyboard.
    <div
      role="presentation"
      className="fixed inset-0 z-40"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) close();
      }}
    >
      <dialog
        open
        aria-modal="true"
        aria-label={placeholder}
        className="fade-in absolute top-1/3 left-1/2 m-0 w-palette -translate-x-1/2 -translate-y-1/3 overflow-hidden rounded-lg border-0 bg-palette-bg p-0 text-palette-text shadow-float"
      >
        <Command
          shouldFilter={!fromRust}
          {...(fromRust ? { value: selected, onValueChange: setPicked } : {})}
          loop
        >
          <Command.Input
            autoFocus
            value={query}
            onValueChange={setQuery}
            placeholder={placeholder}
            className="h-12 w-full border-b border-palette-hi bg-transparent px-4 text-title text-palette-text outline-none placeholder:text-palette-text-2"
          />
          <Command.List className="max-h-palette-list overflow-y-auto py-1">
            <Command.Empty className="px-4 py-2 text-row text-palette-text-2">
              No results
            </Command.Empty>
            {items.map((it) => (
              <Command.Item
                key={it.id}
                value={itemValue(it)}
                onSelect={() => {
                  const start = performance.now();
                  close();
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
                className="mx-1 flex h-row cursor-default items-center justify-between rounded-md px-3 text-row"
              >
                <span className="truncate">{it.label}</span>
                {it.keys && it.keys.length > 0 && <Chip keys={it.keys} />}
              </Command.Item>
            ))}
          </Command.List>
        </Command>
      </dialog>
    </div>
  );
}
