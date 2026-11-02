import { describe, expect, it } from "vite-plus/test";

import type { AccountSummary, ChannelMessage, ListRow, ModifyAction, SubKey } from "../ipc/types";
import {
  addPending,
  applyChannel,
  emptyMirror,
  mergeRow,
  subForKey,
  undoTarget,
  viewList,
  type MirrorState,
} from "./mirror-core";

const inbox: SubKey = { type: "list", account: 1, view: { kind: "inbox" } };
const starred: SubKey = { type: "list", account: 1, view: { kind: "starred" } };

const row = (key: string, patch: Partial<ListRow> = {}): ListRow => ({
  key,
  thread: Number(key),
  draft: null,
  sender: "Ada",
  senderEmail: "ada@example.com",
  subject: "Hi",
  snippet: "",
  time: 0,
  unread: true,
  starred: false,
  hasAttachments: false,
  messageCount: 1,
  labels: ["INBOX", "UNREAD"],
  snoozeUntil: null,
  split: null,
  rev: 1,
  ...patch,
});

function withList(sub: string, key: SubKey, keys: string[], rev = 1): MirrorState {
  let s = emptyMirror();
  s = { ...s, subs: { [sub]: subForKey(key) } };
  const msg: ChannelMessage = {
    rev,
    ack: 0,
    updates: [
      {
        type: "list",
        sub,
        first: true,
        ids: keys.map((k) => ({ key: k, split: null })),
        removed: [],
        moved: [],
        rows: keys.map((k) => row(k)),
        searching: false,
        more: false,
      },
    ],
  };
  return applyChannel(s, msg);
}

const modify = (
  kind: ModifyAction["kind"],
  threads: number[],
  add: string[],
  remove: string[],
): ModifyAction => ({
  account: 1,
  kind,
  threads,
  add,
  remove,
  snooze: null,
  undoable: true,
});

const keys = (s: MirrorState, sub: string) => viewList(s, sub).ids.map((e) => e.key);

describe("channel messages", () => {
  it("applies a first load and then patches", () => {
    let s = withList("s1", inbox, ["1", "2", "3"]);
    s = applyChannel(s, {
      rev: 2,
      ack: 0,
      updates: [
        {
          type: "list",
          sub: "s1",
          first: false,
          ids: null,
          removed: ["2"],
          moved: [{ key: "4", split: null, after: "1" }],
          rows: [row("4", { rev: 2 })],
          searching: false,
          more: false,
        },
      ],
    });
    expect(keys(s, "s1")).toEqual(["1", "4", "3"]);
  });

  it("ignores patches until the first load arrives", () => {
    let s: MirrorState = { ...emptyMirror(), subs: { s1: subForKey(inbox) } };
    s = applyChannel(s, {
      rev: 2,
      ack: 0,
      updates: [
        {
          type: "list",
          sub: "s1",
          first: false,
          ids: null,
          removed: [],
          moved: [{ key: "9", split: null, after: null }],
          rows: [],
          searching: false,
          more: false,
        },
      ],
    });
    expect(keys(s, "s1")).toEqual([]);
  });
});

describe("pending actions", () => {
  it("re-applies actions after ack and drops acked ones", () => {
    let s = withList("s1", inbox, ["1", "2", "3"]);
    s = addPending(s, { n: 5, action: { type: "modify", ...modify("done", [2], [], ["INBOX"]) } });
    expect(keys(s, "s1")).toEqual(["1", "3"]);
    // A poll before Rust applied the action: the row stays gone.
    s = applyChannel(s, { rev: 2, ack: 4, updates: [] });
    expect(keys(s, "s1")).toEqual(["1", "3"]);
    expect(s.pending).toHaveLength(1);
    // Rust confirms: its patch removes the row and the action is dropped.
    s = applyChannel(s, {
      rev: 3,
      ack: 5,
      updates: [
        {
          type: "list",
          sub: "s1",
          first: false,
          ids: null,
          removed: ["2"],
          moved: [],
          rows: [],
          searching: false,
          more: false,
        },
      ],
    });
    expect(s.pending).toHaveLength(0);
    expect(keys(s, "s1")).toEqual(["1", "3"]);
  });

  it("falls back to Rust's state when an action is rejected", () => {
    let s = withList("s1", inbox, ["1", "2"]);
    s = addPending(s, {
      n: 1,
      action: { type: "modify", ...modify("trash", [1], ["TRASH"], ["INBOX"]) },
    });
    expect(keys(s, "s1")).toEqual(["2"]);
    s = applyChannel(s, { rev: 2, ack: 1, updates: [] });
    expect(keys(s, "s1")).toEqual(["1", "2"]);
  });

  it("follows the removal table", () => {
    let s = withList("s1", inbox, ["1", "2"]);
    s = {
      ...s,
      subs: {
        ...s.subs,
        s2: { ...subForKey(starred), loaded: true, ids: [{ key: "1", split: null }] } as never,
      },
    };
    s = addPending(s, { n: 1, action: { type: "modify", ...modify("done", [1], [], ["INBOX"]) } });
    expect(keys(s, "s1")).toEqual(["2"]);
    expect(keys(s, "s2")).toEqual(["1"]);
    s = addPending(s, {
      n: 2,
      action: { type: "modify", ...modify("unstar", [1], [], ["STARRED"]) },
    });
    expect(keys(s, "s2")).toEqual([]);
  });

  it("patches row fields in the same frame", () => {
    let s = withList("s1", inbox, ["1"]);
    s = addPending(s, { n: 1, action: { type: "modify", ...modify("read", [1], [], ["UNREAD"]) } });
    expect(viewList(s, "s1").rows["1"]?.unread).toBe(false);
    s = addPending(s, {
      n: 2,
      action: { type: "modify", ...modify("star", [1], ["STARRED"], []) },
    });
    expect(viewList(s, "s1").rows["1"]?.starred).toBe(true);
  });
});

describe("rev rule", () => {
  it("keeps a row unless ours is newer", () => {
    const rows: Record<string, ListRow> = { "1": row("1", { rev: 5, subject: "new" }) };
    mergeRow(rows, row("1", { rev: 3, subject: "stale" }));
    expect(rows["1"]?.subject).toBe("new");
    mergeRow(rows, row("1", { rev: 6, subject: "newer" }));
    expect(rows["1"]?.subject).toBe("newer");
  });
});

describe("undo target", () => {
  const summary = (undo: number[]): AccountSummary =>
    ({
      account: { id: 1 },
      undo: undo.map((id) => ({ id, kind: "mail", label: "", threads: [], outbox: null })),
    }) as unknown as AccountSummary;

  it("takes the newest entry with pending actions applied", () => {
    let s = withList("s1", inbox, ["1", "2"]);
    s = {
      ...s,
      subs: {
        ...s.subs,
        a: { type: "account", key: { type: "account", account: 1 }, summary: summary([3]) },
      },
    };
    expect(undoTarget(s, 1)).toBe(3);
    s = addPending(s, { n: 7, action: { type: "modify", ...modify("done", [1], [], ["INBOX"]) } });
    expect(undoTarget(s, 1)).toBe(7);
    // A quick second z undoes the entry before.
    s = addPending(s, { n: 8, action: { type: "undo", account: 1, entry: 7 } });
    expect(undoTarget(s, 1)).toBe(3);
  });

  it("puts a removed row back where it was on a pending undo", () => {
    let s = withList("s1", inbox, ["1", "2", "3"]);
    s = addPending(s, { n: 4, action: { type: "modify", ...modify("done", [2], [], ["INBOX"]) } });
    expect(keys(s, "s1")).toEqual(["1", "3"]);
    s = applyChannel(s, {
      rev: 2,
      ack: 4,
      updates: [
        {
          type: "list",
          sub: "s1",
          first: false,
          ids: null,
          removed: ["2"],
          moved: [],
          rows: [],
          searching: false,
          more: false,
        },
      ],
    });
    s = addPending(s, { n: 5, action: { type: "undo", account: 1, entry: 4 } });
    expect(keys(s, "s1")).toEqual(["1", "2", "3"]);
  });

  it("ignores read-on-open, which is not an undo entry", () => {
    let s = withList("s1", inbox, ["1"]);
    s = addPending(s, {
      n: 2,
      action: { type: "modify", ...modify("read", [1], [], ["UNREAD"]), undoable: false },
    });
    expect(undoTarget(s, 1)).toBeNull();
  });
});
