import { expect, test } from "vite-plus/test";

import type {
  Action,
  ChannelMessage,
  ListEntry,
  ListMove,
  ListRow,
  ModifyAction,
  SubKey,
} from "../ipc/types";
import {
  addPending,
  applyChannel,
  dropUnheldRows,
  emptyMirror,
  mergeRow,
  subForKey,
  undoTarget,
  viewList,
  type MirrorState,
} from "./mirror-core";

function makeRng(seed: number): () => number {
  let x = seed >>> 0;
  return () => {
    x ^= x << 13;
    x ^= x >>> 17;
    x ^= x << 5;
    return x >>> 0;
  };
}

function makeRow(id: number, rev: number, labels: string[]): ListRow {
  return {
    key: String(id),
    thread: id,
    draft: null,
    split: 1,
    sender: `Sender ${id}`,
    senderEmail: `s${id}@x.test`,
    subject: `Subject ${id}`,
    snippet: `Snippet ${id}`,
    time: 1_000 + id * 10,
    messageCount: 1,
    unread: labels.includes("UNREAD"),
    starred: labels.includes("STARRED"),
    hasAttachments: false,
    labels,
    snoozeUntil: null,
    rev,
  };
}

function diffEntries(
  oldList: ListEntry[],
  newList: ListEntry[],
): {
  removed: string[];
  moved: ListMove[];
} {
  const newKeys = new Set(newList.map((e) => e.key));
  const removed = oldList.filter((e) => !newKeys.has(e.key)).map((e) => e.key);
  const moved: ListMove[] = newList.map((e, i) => ({
    key: e.key,
    after: i === 0 ? null : (newList[i - 1]?.key ?? null),
    split: e.split,
  }));
  return { removed, moved };
}

type ServerModel = {
  rev: number;
  ack: number;
  rows: Map<number, ListRow>;
  order: number[];
  undo: { id: number; prevLabels: Map<number, string[]> }[];
};

function serverInboxIds(srv: ServerModel): ListEntry[] {
  return srv.order
    .filter((id) => {
      const r = srv.rows.get(id);
      return r != null && r.labels.includes("INBOX") && !r.labels.includes("TRASH");
    })
    .map((id) => ({ key: String(id), split: 1 }));
}

function applyActionToServer(srv: ServerModel, n: number, action: Action): ListRow[] {
  srv.rev += 1;
  srv.ack = Math.max(srv.ack, n);
  const touched: ListRow[] = [];
  if (action.type === "modify") {
    const prevLabels = new Map<number, string[]>();
    for (const tid of action.threads) {
      const cur = srv.rows.get(tid);
      if (!cur) continue;
      prevLabels.set(tid, [...cur.labels]);
      const next = new Set(cur.labels);
      for (const l of action.remove) next.delete(l);
      for (const l of action.add) next.add(l);
      const updated = makeRow(tid, srv.rev, [...next]);
      srv.rows.set(tid, updated);
      touched.push(updated);
    }
    if (action.undoable) srv.undo.push({ id: n, prevLabels });
  } else if (action.type === "undo") {
    const idx = srv.undo.findIndex((u) => u.id === action.entry);
    if (idx >= 0) {
      const [entry] = srv.undo.splice(idx, 1);
      if (entry) {
        for (const [tid, labels] of entry.prevLabels) {
          const updated = makeRow(tid, srv.rev, labels);
          srv.rows.set(tid, updated);
          touched.push(updated);
        }
      }
    }
  }
  return touched;
}

function randomModify(next: () => number, tid: number): ModifyAction {
  const roll = next() % 5;
  switch (roll) {
    case 0:
      return {
        account: 1,
        kind: "done",
        threads: [tid],
        add: [],
        remove: ["INBOX"],
        snooze: null,
        undoable: true,
      };
    case 1:
      return {
        account: 1,
        kind: "trash",
        threads: [tid],
        add: ["TRASH"],
        remove: ["INBOX"],
        snooze: null,
        undoable: true,
      };
    case 2:
      return {
        account: 1,
        kind: "star",
        threads: [tid],
        add: ["STARRED"],
        remove: [],
        snooze: null,
        undoable: true,
      };
    case 3:
      return {
        account: 1,
        kind: "read",
        threads: [tid],
        add: [],
        remove: ["UNREAD"],
        snooze: null,
        undoable: true,
      };
    default:
      return {
        account: 1,
        kind: "unread",
        threads: [tid],
        add: ["UNREAD"],
        remove: [],
        snooze: null,
        undoable: false,
      };
  }
}

function makeSubChannel(
  srv: ServerModel,
  prevIds: ListEntry[],
  touched: ListRow[],
  first: boolean,
): { msg: ChannelMessage; ids: ListEntry[] } {
  const ids = serverInboxIds(srv);
  const patch = first ? { removed: [], moved: [] } : diffEntries(prevIds, ids);
  const msg: ChannelMessage = {
    rev: srv.rev,
    ack: srv.ack,
    rejected: [],
    updates: [
      {
        type: "list",
        sub: "inbox",
        first,
        ids: first ? ids : null,
        removed: patch.removed,
        moved: patch.moved,
        rows: touched,
        searching: false,
        needsGmail: false,
        looksInTrash: false,
        terms: [],
        more: false,
        oldestHit: null,
      },
      {
        type: "account",
        sub: "acct",
        first,
        summary: {
          account: {
            id: 1,
            email: "a@x.test",
            name: "A",
            hue: "0",
            inboxUnread: 0,
            needsSignin: false,
            muted: false,
            syncStatus: "idle",
            sendAs: [],
          },
          labels: [],
          splits: [],
          outbox: [],
          undo: srv.undo.map((u) => ({ id: u.id, outbox: null })),
          notices: [],
          queuedOps: 0,
        },
      },
    ],
  };
  return { msg, ids };
}

test("mirror preserves unacked removals across partial acks and converges to server truth", () => {
  const inboxKey: SubKey = { type: "list", account: 1, view: { kind: "inbox" } };
  const acctKey: SubKey = { type: "account", account: 1 };

  for (let seed = 1; seed <= 25; seed++) {
    const next = makeRng(seed * 9973);
    const srv: ServerModel = { rev: 1, ack: 0, rows: new Map(), order: [], undo: [] };
    for (let id = 1; id <= 8; id++) {
      srv.order.push(id);
      srv.rows.set(id, makeRow(id, 1, ["INBOX", "UNREAD"]));
    }

    let mirror: MirrorState = {
      ...emptyMirror(),
      subs: { inbox: subForKey(inboxKey), acct: subForKey(acctKey) },
    };
    const init = makeSubChannel(srv, [], [...srv.rows.values()], true);
    let lastSentIds = init.ids;
    mirror = applyChannel(mirror, init.msg, new Set());

    let nextN = 1;
    const queue: { n: number; action: Action }[] = [];

    const ackViolations: string[] = [];
    const flashViolations: string[] = [];

    for (let step = 0; step < 30; step++) {
      const choice = next() % 4;
      if (choice <= 1) {
        const visible = viewList(mirror, "inbox").ids;
        const n = nextN++;
        let action: Action | null = null;
        if (choice === 1) {
          const target = undoTarget(mirror, 1);
          if (target != null) action = { type: "undo", account: 1, entry: target };
        }
        if (action == null && visible.length > 0) {
          const picked = visible[next() % visible.length];
          if (picked) action = { type: "modify", ...randomModify(next, Number(picked.key)) };
        }
        if (action != null) {
          mirror = addPending(mirror, { n, action });
          queue.push({ n, action });
        }
      } else if (choice === 2 && queue.length > 0) {
        const prevAck = mirror.ack;
        const item = queue.shift();
        if (item) {
          const touched = applyActionToServer(srv, item.n, item.action);
          const { msg, ids } = makeSubChannel(srv, lastSentIds, touched, false);
          lastSentIds = ids;
          mirror = applyChannel(mirror, msg, new Set());
          if (mirror.ack < prevAck) {
            ackViolations.push(`seed=${seed} step=${step} ack ${mirror.ack} < ${prevAck}`);
          }
          for (const rem of mirror.pending) {
            if (
              rem.action.type === "modify" &&
              (rem.action.kind === "done" || rem.action.kind === "trash")
            ) {
              const afterView = new Set(viewList(mirror, "inbox").ids.map((e) => e.key));
              const laterUndone = mirror.pending.some(
                (p) => p.n > rem.n && p.action.type === "undo" && p.action.entry === rem.n,
              );
              if (!laterUndone) {
                for (const tid of rem.action.threads) {
                  if (afterView.has(String(tid))) {
                    flashViolations.push(`seed=${seed} step=${step} thread=${tid} flashed back`);
                  }
                }
              }
            }
          }
        }
      } else {
        const newId = 100 + step;
        srv.rev += 1;
        const fresh = makeRow(newId, srv.rev, ["INBOX", "UNREAD"]);
        srv.rows.set(newId, fresh);
        srv.order.unshift(newId);
        const { msg, ids } = makeSubChannel(srv, lastSentIds, [fresh], false);
        lastSentIds = ids;
        mirror = applyChannel(mirror, msg, new Set());
      }
    }

    expect(ackViolations).toEqual([]);
    expect(flashViolations).toEqual([]);

    while (queue.length > 0) {
      const item = queue.shift();
      if (!item) break;
      const touched = applyActionToServer(srv, item.n, item.action);
      const { msg, ids } = makeSubChannel(srv, lastSentIds, touched, false);
      lastSentIds = ids;
      mirror = applyChannel(mirror, msg, new Set());
    }

    expect(mirror.pending).toEqual([]);
    expect(mirror.ack).toBe(srv.ack);
    const finalView = viewList(mirror, "inbox");
    expect(finalView.ids).toEqual(serverInboxIds(srv));
    for (const entry of finalView.ids) {
      const serverRow = srv.rows.get(Number(entry.key));
      expect(finalView.rows[entry.key]?.unread).toBe(serverRow?.unread);
      expect(finalView.rows[entry.key]?.starred).toBe(serverRow?.starred);
    }
    const expectedUndo = srv.undo.length > 0 ? Math.max(...srv.undo.map((u) => u.id)) : null;
    expect(undoTarget(mirror, 1)).toBe(expectedUndo);
  }
});

test("mergeRow ignores stale revisions and dropUnheldRows keeps referenced rows", () => {
  const rows: Record<string, ListRow> = {};
  mergeRow(rows, makeRow(1, 5, ["INBOX"]));
  mergeRow(rows, makeRow(1, 3, ["TRASH"]));
  expect(rows["1"]?.rev).toBe(5);
  expect(rows["1"]?.labels).toEqual(["INBOX"]);

  mergeRow(rows, makeRow(2, 1, ["INBOX"]));
  mergeRow(rows, makeRow(3, 1, ["INBOX"]));
  const inboxSub = subForKey({ type: "list", account: 1, view: { kind: "inbox" } });
  if (inboxSub.type === "list") inboxSub.ids = [{ key: "1", split: 1 }];
  dropUnheldRows(rows, { inbox: inboxSub }, {}, ["1", "2", "3"], new Set(["2"]));
  expect(Object.keys(rows).toSorted()).toEqual(["1", "2"]);
});
