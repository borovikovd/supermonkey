import type {
  AccountSummary,
  Action,
  AppState,
  ChannelMessage,
  ConversationData,
  DraftState,
  ListEntry,
  ListMove,
  ListRow,
  ModifyAction,
  SubKey,
  View,
} from "../ipc/types";

type KeyOf<T extends SubKey["type"]> = Extract<SubKey, { type: T }>;

type ListSub = {
  type: "list";
  key: KeyOf<"list" | "search">;
  loaded: boolean;
  ids: ListEntry[];
  searching: boolean;
  needsGmail: boolean;
  looksInTrash: boolean;
  terms: string[];
  more: boolean;
  oldestHit: string | null;
};
type ConversationSub = {
  type: "conversation";
  key: KeyOf<"conversation">;
  data: ConversationData | null;
};
type ComposerSub = { type: "composer"; key: KeyOf<"composer">; draft: DraftState | null };
type AccountSub = { type: "account"; key: KeyOf<"account">; summary: AccountSummary | null };
type AppSub = { type: "app"; key: KeyOf<"app">; state: AppState | null };
export type Sub = ListSub | ConversationSub | ComposerSub | AccountSub | AppSub;

export type Pending = { n: number; action: Action };

type Removal = { sub: string; key: string; after: string | null };

export type MirrorState = {
  ack: number;
  subs: Record<string, Sub>;
  rows: Record<string, ListRow>;
  pending: Pending[];
  removals: Record<number, Removal[]>;
};

export const emptyMirror = (): MirrorState => ({
  ack: 0,
  subs: {},
  rows: {},
  pending: [],
  removals: {},
});

export function subForKey(key: SubKey): Sub {
  switch (key.type) {
    case "list":
    case "search":
      return {
        type: "list",
        key,
        loaded: false,
        ids: [],
        searching: false,
        needsGmail: false,
        looksInTrash: false,
        terms: [],
        more: false,
        oldestHit: null,
      };
    case "conversation":
      return { type: "conversation", key, data: null };
    case "composer":
      return { type: "composer", key, draft: null };
    case "account":
      return { type: "account", key, summary: null };
    case "app":
      return { type: "app", key, state: null };
  }
}

export function mergeRow(rows: Record<string, ListRow>, row: ListRow): void {
  const have = rows[row.key];
  if (!have || have.rev <= row.rev) rows[row.key] = row;
}

function applyListPatch(ids: ListEntry[], removed: string[], moved: ListMove[]): ListEntry[] {
  if (removed.length === 0 && moved.length === 0) return ids;
  const drop = new Set<string>(removed);
  for (const m of moved) drop.add(m.key);
  const out = ids.filter((e) => !drop.has(e.key));
  for (const m of moved) {
    let at = 0;
    if (m.after !== null) {
      const pos =
        out.at(-1)?.key === m.after ? out.length - 1 : out.findIndex((e) => e.key === m.after);
      at = pos >= 0 ? pos + 1 : out.length;
    }
    out.splice(at, 0, { key: m.key, split: m.split });
  }
  return out;
}

export function dropUnheldRows(
  rows: Record<string, ListRow>,
  subs: Record<string, Sub>,
  removals: Record<number, Removal[]>,
  keys: Iterable<string>,
  keep: ReadonlySet<string>,
): void {
  let held: Set<string> | null = null;
  for (const k of keys) {
    if (!(k in rows) || keep.has(k)) continue;
    held ??= heldKeys(subs, removals);
    if (!held.has(k)) delete rows[k];
  }
}

function heldKeys(subs: Record<string, Sub>, removals: Record<number, Removal[]>): Set<string> {
  const held = new Set<string>();
  for (const sub of Object.values(subs)) {
    if (sub.type === "list") for (const e of sub.ids) held.add(e.key);
    else if (sub.type === "conversation") held.add(String(sub.key.thread));
  }
  for (const list of Object.values(removals)) for (const r of list) held.add(r.key);
  return held;
}

export function applyChannel(
  s: MirrorState,
  msg: ChannelMessage,
  keep: ReadonlySet<string>,
): MirrorState {
  const subs = { ...s.subs };
  const lists = msg.updates.some((u) => u.type === "list");
  const rows = lists ? { ...s.rows } : s.rows;
  const left: string[] = [];
  for (const u of msg.updates) {
    const sub = subs[u.sub];
    if (!sub) continue;
    switch (u.type) {
      case "list": {
        if (sub.type !== "list") break;
        if (!u.first && !sub.loaded) break;
        for (const r of u.rows) mergeRow(rows, r);
        const ids = u.first ? (u.ids ?? []) : applyListPatch(sub.ids, u.removed, u.moved);
        if (u.first) left.push(...sub.ids.map((e) => e.key));
        else left.push(...u.removed);
        subs[u.sub] = {
          ...sub,
          loaded: true,
          ids,
          searching: u.searching,
          needsGmail: u.needsGmail,
          looksInTrash: u.looksInTrash,
          terms:
            u.terms.length === sub.terms.length && u.terms.every((t, i) => t === sub.terms[i])
              ? sub.terms
              : u.terms,
          more: u.more,
          oldestHit: u.oldestHit,
        };
        break;
      }
      case "conversation":
        if (sub.type === "conversation" && (u.first || sub.data))
          subs[u.sub] = { ...sub, data: u.data };
        break;
      case "composer":
        if (sub.type === "composer" && (u.first || sub.draft))
          subs[u.sub] = { ...sub, draft: u.draft };
        break;
      case "account":
        if (sub.type === "account" && (u.first || sub.summary))
          subs[u.sub] = { ...sub, summary: u.summary };
        break;
      case "app":
        if (sub.type === "app" && (u.first || sub.state)) subs[u.sub] = { ...sub, state: u.state };
        break;
    }
  }
  if (lists) dropUnheldRows(rows, subs, s.removals, left, keep);
  return {
    ...s,
    subs,
    rows,
    pending: s.pending.some((p) => p.n <= msg.ack)
      ? s.pending.filter((p) => p.n > msg.ack)
      : s.pending,
    ack: Math.max(s.ack, msg.ack),
  };
}

export const viewLabel = (view: View): string | null => {
  switch (view.kind) {
    case "inbox":
      return "INBOX";
    case "starred":
      return "STARRED";
    case "sent":
      return "SENT";
    case "trash":
      return "TRASH";
    case "spam":
      return "SPAM";
    case "label":
      return view.label;
    case "done":
    case "allMail":
    case "drafts":
    case "snoozed":
      return null;
  }
};

function removesFrom(m: ModifyAction, key: SubKey): boolean {
  if (key.type === "search") {
    return m.kind === "trash" || m.kind === "spam";
  }
  if (key.type !== "list") return false;
  const v = key.view.kind;
  switch (m.kind) {
    case "done":
      return v === "inbox" || v === "snoozed";
    case "returnToInbox":
      return v === "done" || v === "snoozed" || v === "trash" || v === "spam";
    case "snooze":
      return v === "inbox" || v === "done";
    case "trash":
      return v !== "trash";
    case "spam":
      return v !== "spam";
    case "unstar":
      return v === "starred";
    case "removeLabel": {
      const label = viewLabel(key.view);
      return label !== null && m.remove.includes(label);
    }
    case "label":
    case "read":
    case "star":
    case "unread":
      return false;
  }
}

function modifyOf(a: Action): ModifyAction | null {
  if (a.type === "modify") return a;
  if (a.type === "createLabel") return a.apply;
  return null;
}

function flag(m: ModifyAction, label: string, current: boolean): boolean {
  if (m.add.includes(label)) return true;
  if (m.remove.includes(label)) return false;
  return current;
}

function patchRow(row: ListRow, m: ModifyAction): ListRow {
  const labels = new Set(row.labels);
  for (const l of m.remove) labels.delete(l);
  for (const l of m.add) labels.add(l);
  let snoozeUntil = row.snoozeUntil;
  if (m.snooze?.kind === "set") snoozeUntil = m.snooze.at;
  else if (
    m.snooze?.kind === "clear" ||
    m.kind === "done" ||
    m.kind === "trash" ||
    m.kind === "spam"
  ) {
    snoozeUntil = null;
  }
  return {
    ...row,
    labels: [...labels],
    unread: flag(m, "UNREAD", row.unread),
    starred: flag(m, "STARRED", row.starred),
    snoozeUntil,
  };
}

export function composerDraft(s: MirrorState, draft: string): DraftState | null {
  for (const sub of Object.values(s.subs)) {
    if (sub.type === "composer" && sub.key.draft === draft) return sub.draft;
  }
  return null;
}

export function appState(s: MirrorState): AppState | null {
  for (const sub of Object.values(s.subs)) if (sub.type === "app") return sub.state;
  return null;
}

export function accountSummary(s: MirrorState, account: number): AccountSummary | null {
  for (const sub of Object.values(s.subs)) {
    if (sub.type === "account" && sub.key.account === account) return sub.summary;
  }
  return null;
}

export function undoTarget(s: MirrorState, account: number): number | null {
  const entries = (accountSummary(s, account)?.undo ?? []).map((e) => e.id);
  for (const { n, action: a } of s.pending) {
    if (a.type === "undo" && a.account === account) {
      const i = entries.indexOf(a.entry);
      if (i >= 0) entries.splice(i, 1);
      continue;
    }
    const addsEntry = (a.type === "modify" && a.undoable) || a.type === "pin";
    if (addsEntry && a.account === account) entries.push(n);
  }
  return entries.length > 0 ? Math.max(...entries) : null;
}

export type ListView = { ids: ListEntry[]; rows: Record<string, ListRow> };

export function viewList(s: MirrorState, subId: string): ListView {
  const sub = s.subs[subId];
  if (!sub || sub.type !== "list") return { ids: [], rows: s.rows };
  let ids = sub.ids;
  let rows = s.rows;
  let copied = false;
  const own = () => {
    if (!copied) {
      rows = { ...rows };
      copied = true;
    }
  };
  for (const { action: a } of s.pending) {
    const m = modifyOf(a);
    if (m) {
      for (const t of m.threads) {
        const k = String(t);
        const row = rows[k];
        if (row) {
          own();
          rows[k] = patchRow(row, m);
        }
      }
      if (removesFrom(m, sub.key)) {
        const drop = new Set(m.threads.map(String));
        ids = ids.filter((e) => !drop.has(e.key));
      }
    } else if (a.type === "undo") {
      const back = s.removals[a.entry]?.filter((r) => r.sub === subId) ?? [];
      for (const r of back) {
        if (ids.some((e) => e.key === r.key)) continue;
        const at = r.after === null ? 0 : ids.findIndex((e) => e.key === r.after) + 1;
        const split = rows[r.key]?.split ?? null;
        ids = [...ids.slice(0, at), { key: r.key, split }, ...ids.slice(at)];
      }
    } else if (
      (a.type === "sendDraft" || a.type === "discardDraft") &&
      sub.key.type === "list" &&
      sub.key.view.kind === "drafts"
    ) {
      ids = ids.filter((e) => e.key !== a.draft);
    }
  }
  return { ids, rows };
}

function recordRemovals(s: MirrorState, p: Pending): Removal[] {
  const m = modifyOf(p.action);
  if (!m?.undoable) return [];
  const drop = new Set(m.threads.map(String));
  const out: Removal[] = [];
  for (const [subId, sub] of Object.entries(s.subs)) {
    if (sub.type !== "list" || !removesFrom(m, sub.key)) continue;
    const { ids } = viewList(s, subId);
    let after: string | null = null;
    for (const e of ids) {
      if (drop.has(e.key)) out.push({ sub: subId, key: e.key, after });
      else after = e.key;
    }
  }
  return out;
}

export function addPending(s: MirrorState, p: Pending): MirrorState {
  const removed = recordRemovals(s, p);
  const removals = removed.length > 0 ? { ...s.removals, [p.n]: removed } : s.removals;
  return { ...s, pending: [...s.pending, p], removals };
}
