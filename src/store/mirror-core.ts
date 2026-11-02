// The mirror's pure logic (SPEC UI state). The Zustand store in mirror.ts
// wraps it. Kept free of React and Tauri so Vitest can drive it directly.
//
// The mirror holds the state Rust confirmed plus the actions after `ack`, and
// shows those actions applied on top. A channel message replaces confirmed
// state, drops actions up to its `ack`, and re-applies the rest.

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
  UndoEntryInfo,
  View,
} from "../ipc/types";

export type ListSub = {
  type: "list";
  key: SubKey;
  loaded: boolean;
  ids: ListEntry[];
  searching: boolean;
  /** Gmail has another page (Search, Label, Trash, Spam). */
  more: boolean;
};
export type ConversationSub = { type: "conversation"; key: SubKey; data: ConversationData | null };
export type ComposerSub = { type: "composer"; key: SubKey; draft: DraftState | null };
export type AccountSub = { type: "account"; key: SubKey; summary: AccountSummary | null };
export type AppSub = { type: "app"; key: SubKey; state: AppState | null };
export type Sub = ListSub | ConversationSub | ComposerSub | AccountSub | AppSub;

export type Pending = { n: number; action: Action };

/** Rows removed from a list by an action made this session, for undo. */
export type Removal = { sub: string; key: string; after: string | null };

export type MirrorState = {
  ack: number;
  rev: number;
  subs: Record<string, Sub>;
  /** Confirmed rows by key, shared by every list. */
  rows: Record<string, ListRow>;
  pending: Pending[];
  /** Per undo entry id (= action number): rows its action removed from lists. */
  removals: Record<number, Removal[]>;
};

export const emptyMirror = (): MirrorState => ({
  ack: 0,
  rev: 0,
  subs: {},
  rows: {},
  pending: [],
  removals: {},
});

export function subForKey(key: SubKey): Sub {
  switch (key.type) {
    case "list":
    case "search":
      return { type: "list", key, loaded: false, ids: [], searching: false, more: false };
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

/** Keeps a row unless our copy has a newer `rev`. */
export function mergeRow(rows: Record<string, ListRow>, row: ListRow): void {
  const have = rows[row.key];
  if (!have || have.rev <= row.rev) rows[row.key] = row;
}

export function applyListPatch(
  ids: ListEntry[],
  removed: string[],
  moved: ListMove[],
): ListEntry[] {
  if (removed.length === 0 && moved.length === 0) return ids;
  const drop = new Set<string>(removed);
  for (const m of moved) drop.add(m.key);
  const out = ids.filter((e) => !drop.has(e.key));
  const index = new Map<string, number>();
  out.forEach((e, i) => index.set(e.key, i));
  for (const m of moved) {
    let at = 0;
    if (m.after !== null) {
      // Moves arrive in new-list order, so `after` is usually just placed.
      const prev = out.at(-1)?.key === m.after ? out.length - 1 : -1;
      const pos = prev >= 0 ? prev : out.findIndex((e) => e.key === m.after);
      at = pos >= 0 ? pos + 1 : out.length;
    }
    out.splice(at, 0, { key: m.key, split: m.split });
  }
  return out;
}

/** Applies one channel message to confirmed state and drops acked actions. */
export function applyChannel(s: MirrorState, msg: ChannelMessage): MirrorState {
  const subs = { ...s.subs };
  const rows = { ...s.rows };
  for (const u of msg.updates) {
    const sub = subs[u.sub];
    // Ignore patches until the subscription's first load arrives.
    if (!sub) continue;
    switch (u.type) {
      case "list": {
        if (sub.type !== "list") break;
        if (!u.first && !sub.loaded) break;
        for (const r of u.rows) mergeRow(rows, r);
        const ids = u.first ? (u.ids ?? []) : applyListPatch(sub.ids, u.removed, u.moved);
        subs[u.sub] = { ...sub, loaded: true, ids, searching: u.searching, more: u.more };
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
  const pending = s.pending.filter((p) => p.n > msg.ack);
  // Drop session removals for undo entries that are gone from every account summary.
  return {
    ...s,
    subs,
    rows,
    pending,
    ack: Math.max(s.ack, msg.ack),
    rev: Math.max(s.rev, msg.rev),
  };
}

// ------------------------------------------------------------ pending effects

const viewLabel = (view: View): string | null => {
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
    default:
      return null;
  }
};

/** Whether an action takes a row out of a view, per SPEC UI state → Actions. */
export function removesFrom(m: ModifyAction, key: SubKey): boolean {
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
    default:
      return false;
  }
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
    unread: m.add.includes("UNREAD") ? true : m.remove.includes("UNREAD") ? false : row.unread,
    starred: m.add.includes("STARRED") ? true : m.remove.includes("STARRED") ? false : row.starred,
    snoozeUntil,
  };
}

/** The undo entry a pending action adds, so `z` can target it before Rust confirms. */
function pendingEntry(p: Pending): UndoEntryInfo | null {
  const a = p.action;
  if (a.type === "modify" && a.undoable) {
    return { id: p.n, kind: "mail", label: "", threads: a.threads, outbox: null };
  }
  if (a.type === "pin") return { id: p.n, kind: "pin", label: "", threads: [], outbox: null };
  return null;
}

/** The newest undo entry for an account with pending actions applied. */
export function undoTarget(s: MirrorState, account: number): number | null {
  const summary = Object.values(s.subs).find(
    (x): x is AccountSub => x.type === "account" && x.summary?.account.id === account,
  )?.summary;
  const entries = (summary?.undo ?? []).map((e) => e.id);
  for (const p of s.pending) {
    const a = p.action;
    if (a.type === "undo" && a.account === account) {
      const i = entries.indexOf(a.entry);
      if (i >= 0) entries.splice(i, 1);
      continue;
    }
    const accountOf =
      a.type === "modify"
        ? a.account
        : a.type === "pin"
          ? a.account
          : a.type === "sendDraft"
            ? null
            : null;
    if (accountOf === account && pendingEntry(p)) entries.push(p.n);
  }
  return entries.length > 0 ? Math.max(...entries) : null;
}

export type ListView = { ids: ListEntry[]; rows: Record<string, ListRow> };

/**
 * A list as shown: confirmed ids and rows with pending actions applied.
 * Row field changes apply to every pending modify; removals follow the table;
 * a pending undo of an entry made this session puts its removed rows back.
 */
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
  for (const p of s.pending) {
    const a = p.action;
    const m = a.type === "modify" ? a : a.type === "createLabel" ? a.apply : null;
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

/** Records, for undo, the rows an action removes from each open list. */
export function recordRemovals(s: MirrorState, p: Pending): Removal[] {
  const a = p.action;
  const m = a.type === "modify" ? a : a.type === "createLabel" ? a.apply : null;
  if (!m || !m.undoable) return [];
  const out: Removal[] = [];
  for (const [subId, sub] of Object.entries(s.subs)) {
    if (sub.type !== "list" || !removesFrom(m, sub.key)) continue;
    const { ids } = viewList(s, subId);
    const drop = new Set(m.threads.map(String));
    let after: string | null = null;
    for (const e of ids) {
      if (drop.has(e.key)) out.push({ sub: subId, key: e.key, after });
      else after = e.key;
    }
  }
  return out;
}

/** Adds a pending action; returns the new state. */
export function addPending(s: MirrorState, p: Pending): MirrorState {
  const removed = recordRemovals(s, p);
  const removals = removed.length > 0 ? { ...s.removals, [p.n]: removed } : s.removals;
  return { ...s, pending: [...s.pending, p], removals };
}
