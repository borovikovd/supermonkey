import { Command } from "cmdk";
import { useEffect, useState } from "react";

import {
  applyLabel,
  markDone,
  removeViewLabel,
  returnToInbox,
  snooze,
  unsubscribe,
  block,
  spam,
  targets,
  toggleRead,
  toggleStar,
  trash,
  undo,
} from "../commands";
import { formatAddr } from "../compose/RecipientField";
import { discardOnScreen, draftOnScreen, mountedComposer, sendOnScreen } from "../compose/session";
import { startCompose, startReply } from "../compose/start";
import {
  act,
  addAccount,
  blockedSenders,
  signInAgain,
  snoozeOptions,
  unblockSender,
} from "../ipc/bridge";
import { toast } from "sonner";
import { reportSignIn } from "./signIn";
import type { BlockedSenders, PinKind, SnoozeChoice, View } from "../ipc/types";
import { selectAll } from "../keyboard/nav";
import { measure } from "../keyboard/meter";
import {
  closePalette,
  goTo,
  goToSplit,
  openPalette,
  openSearch,
  openSheet,
  switchAccount,
} from "../navigation";
import { useChrome, type PaletteMode } from "../store/chrome";
import { rowOf } from "../store/current";
import { useAppState, useCurrent, useSummary } from "../store/hooks";

type Item = { id: string; label: string; keys?: string; run: () => void; keep?: boolean };

const close = closePalette;
const view = (v: View) => () => goTo(v);

/** `⌘K` while a composer is blurred. */
const DRAFT_ACTIONS: Item[] = [
  { id: "send", label: "Send", keys: "⌘ ↵", run: sendOnScreen },
  { id: "attach", label: "Attach a file", keys: "⌘ ⇧ A", run: () => mountedComposer()?.attach() },
  { id: "discard", label: "Discard draft", keys: "⌘ ⇧ ,", run: discardOnScreen },
];

function Chip({ keys }: { keys: string }) {
  return (
    <span className="flex gap-1">
      {keys.split(" ").map((k) => (
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
  // while it is open, so the selection cannot move under it.
  const [opened] = useState(() => {
    const threads = targets();
    return { threads, rows: threads.map((id) => rowOf(String(id))) };
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
  const muted = summary?.account.muted ?? false;
  switch (mode.kind) {
    case "commands": {
      // While a composer is on screen, the palette holds its draft actions.
      if (draftOnScreen() !== null) return { items: DRAFT_ACTIONS, placeholder: "Draft actions" };
      const items: Item[] = [
        { id: "done", label: "Mark done", keys: "E", run: markDone },
        { id: "inbox", label: "Return to inbox", keys: "⇧ E", run: returnToInbox },
        {
          id: "snooze",
          label: "Snooze",
          keys: "H",
          run: () => openPalette({ kind: "snooze" }),
          keep: true,
        },
        { id: "trash", label: "Trash", keys: "#", run: trash },
        {
          id: "spam",
          label: "Mark as spam",
          keys: "!",
          run: () => openPalette({ kind: "spam" }),
          keep: true,
        },
        {
          id: "block",
          label: "Block",
          run: () => openPalette({ kind: "block" }),
          keep: true,
        },
        {
          id: "blocked",
          label: "Blocked senders",
          run: () => openPalette({ kind: "blocked" }),
          keep: true,
        },
        { id: "star", label: "Star or unstar", keys: "S", run: toggleStar },
        {
          id: "unsubscribe",
          label: "Unsubscribe",
          keys: "⌘ U",
          run: () => openPalette({ kind: "unsubscribe" }),
          keep: true,
        },
        { id: "read", label: "Mark read or unread", keys: "U", run: toggleRead },
        { id: "select-all", label: "Select all", keys: "⌘ ⇧ A", run: () => selectAll(false) },
        {
          id: "select-from-here",
          label: "Select all from here",
          keys: "⌘ A",
          run: () => selectAll(true),
        },
        {
          id: "label",
          label: "Add or remove label",
          keys: "L",
          run: () => openPalette({ kind: "labels" }),
          keep: true,
        },
        { id: "unlabel", label: "Remove current label", keys: "Y", run: removeViewLabel },
        { id: "undo", label: "Undo", keys: "Z", run: undo },
        { id: "reply", label: "Reply", keys: "R", run: () => startReply("reply") },
        { id: "replyall", label: "Reply all", keys: "↵", run: () => startReply("replyAll") },
        { id: "forward", label: "Forward", keys: "F", run: () => startReply("forward") },
        { id: "compose", label: "Compose", keys: "C", run: startCompose },
        { id: "search", label: "Search", keys: "/", run: openSearch },
        { id: "g-inbox", label: "Go to Inbox", run: view({ kind: "inbox" }) },
        {
          id: "g-imp",
          label: "Go to Important",
          keys: "G I",
          run: () => goToSplit("important"),
        },
        {
          id: "g-other",
          label: "Go to Other",
          keys: "G O",
          run: () => goToSplit("other"),
        },
        { id: "g-done", label: "Go to Done", keys: "G E", run: view({ kind: "done" }) },
        { id: "g-all", label: "Go to All Mail", keys: "G A", run: view({ kind: "allMail" }) },
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
          run: () => openPalette({ kind: "goLabel" }),
          keep: true,
        },
        {
          id: "split-add",
          label: "Add to split",
          run: () => openPalette({ kind: "addToSplit" }),
          keep: true,
        },
        {
          id: "split-imp",
          label: "Move to Important",
          run: () => openPalette({ kind: "addToSplit", preset: "important" }),
          keep: true,
        },
        {
          id: "split-other",
          label: "Move to Other",
          run: () => openPalette({ kind: "addToSplit", preset: "other" }),
          keep: true,
        },
        {
          id: "split-settings",
          label: "Split Inbox settings",
          run: () => openSheet("splits"),
        },
        {
          id: "switch",
          label: "Switch account",
          run: () => openPalette({ kind: "switchAccount" }),
          keep: true,
        },
        { id: "add-account", label: "Add account", run: () => reportSignIn(addAccount()) },
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
          run: () => openSheet("settings"),
        },
        {
          id: "shortcuts",
          label: "Keyboard shortcuts",
          keys: "?",
          run: () => openSheet("shortcuts"),
        },
      ];
      return { items, placeholder: "Type a command" };
    }
    case "labels": {
      const items: Item[] = userLabels.map((l) => {
        const all =
          opened.rows.length > 0 && opened.rows.every((row) => row?.labels.includes(l.id));
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
          run: view({ kind: "label", label: l.id }),
        })),
        placeholder: "Go to label",
      };
    case "snooze":
      return {
        items: (snoozes?.options ?? []).map((o) => ({
          id: `${o.at}`,
          label: o.label,
          run: () => snooze(o.at),
        })),
        placeholder: "Snooze until… (try “fri 9am”)",
        stale: snoozes?.query !== query,
      };
    case "unsubscribe":
      return {
        items: [
          { id: "keep", label: "Unsubscribe", run: () => void unsubscribe("keep") },
          {
            id: "done",
            label: "Unsubscribe, and mark done all",
            run: () => void unsubscribe("done"),
          },
          {
            id: "trash",
            label: "Unsubscribe, and trash all",
            run: () => void unsubscribe("trash"),
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
          ...(asSpam ? [{ id: "spam", label: "Mark spam", run: spam }] : []),
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
          keys: `⌃ ${i + 1}`,
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
  const { items, placeholder, stale } = useCommands(mode, query);
  // Snooze options arrive from Rust, already filtered, after each keystroke:
  // keep the first one selected so Enter always picks something.
  const fromRust = mode.kind === "snooze";
  const [picked, setPicked] = useState("");
  const firstValue = items[0] ? itemValue(items[0]) : "";
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
          onKeyDown={(e) => {
            const start = performance.now();
            if (e.key === "Escape") {
              e.preventDefault();
              e.stopPropagation();
              close();
            } else if (e.key === "Enter" && stale) {
              // The options on screen are for an earlier query: Enter takes the
              // first option for what is typed now, once it arrives.
              e.preventDefault();
              void snoozeOptions(query).then(([first]) => {
                if (!first || useChrome.getState().palette !== mode) return;
                close();
                snooze(first.at);
              });
            } else measure("paletteKey", start);
          }}
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
                className="mx-1 flex h-row cursor-default items-center justify-between rounded-md px-3 text-row"
              >
                <span className="truncate">{it.label}</span>
                {it.keys && <Chip keys={it.keys} />}
              </Command.Item>
            ))}
          </Command.List>
        </Command>
      </dialog>
    </div>
  );
}
