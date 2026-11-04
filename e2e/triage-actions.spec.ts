// Triage actions (COVERAGE 5.5, TRI): archive, return to inbox, trash, spam,
// star, read, selection, the removal table, and the ids an action records.
import type { Route } from "@playwright/test";

import type { App } from "./app.ts";
import type { Control, GmailState } from "./control.ts";
import { expect, test } from "./fixtures.ts";
import * as seeds from "./seeds.ts";
import { A, dates, type Seeded } from "./seeds.ts";

// ------------------------------------------------------------------ helpers

const notDraft = (labels: string[]) => !labels.includes("DRAFT");

/** A thread's non-draft messages in Gmail. */
function threadMessages(state: GmailState, threadId: string) {
  return state.messages.filter((m) => m.threadId === threadId && notDraft(m.labels));
}

/**
 * Whether Gmail puts the thread in a label's view (COVERAGE 1): TRASH and SPAM
 * on any message; anything else on a message outside trash and spam.
 */
function threadHas(state: GmailState, threadId: string, label: string): boolean {
  const msgs = threadMessages(state, threadId);
  if (label === "TRASH" || label === "SPAM") return msgs.some((m) => m.labels.includes(label));
  return msgs.some(
    (m) => !m.labels.includes("TRASH") && !m.labels.includes("SPAM") && m.labels.includes(label),
  );
}

async function has(gmail: Control, s: Seeded, key: string, label: string): Promise<boolean> {
  return threadHas(await gmail.gmail(A), s.thread(key).threadId, label);
}

async function hasEach(gmail: Control, s: Seeded, keys: string[], label: string) {
  const state = await gmail.gmail(A);
  return keys.map((k) => threadHas(state, s.thread(k).threadId, label));
}

/** Runs `step` for each item, one after another. */
async function each<T>(items: readonly T[], step: (item: T) => Promise<unknown>): Promise<void> {
  await items.reduce<Promise<unknown>>(
    (prev, item) => prev.then(() => step(item)),
    Promise.resolve(),
  );
}

/** Moves the highlight with `j` / `k` onto the row containing `text`. */
async function highlight(app: App, text: string): Promise<void> {
  await expect(app.row(text)).toHaveCount(1);
  await expect(app.highlighted).toHaveCount(1);
  const texts = await app.rows.allInnerTexts();
  const target = texts.findIndex((t) => t.includes(text));
  const at = await app.rows.evaluateAll((els) =>
    els.findIndex((el) => el.getAttribute("aria-selected") === "true"),
  );
  const delta = target - at;
  const key = delta >= 0 ? "j" : "k";
  await app.press(...Array.from({ length: Math.abs(delta) }, () => key));
  await expect(app.highlighted).toContainText(text);
}

/** `g` then a key: a view jump. */
async function go(app: App, key: string): Promise<void> {
  await app.press("g", key);
}

/** `g` `l`, then picks a user label by name. */
async function goLabel(app: App, name: string): Promise<void> {
  await app.press("g", "l");
  await expect(app.palette).toBeVisible();
  await app.palette.getByRole("option", { name, exact: true }).click();
  await expect(app.palette).toHaveCount(0);
}

/** `h`, then the first snooze choice (later today). */
async function snoozeHighlighted(app: App): Promise<void> {
  await app.press("h");
  await expect(app.palette.getByRole("option").first()).toBeVisible();
  await app.press("Enter");
  await expect(app.palette).toHaveCount(0);
}

/**
 * Parks the page's action submits, so Rust has not seen the action and only
 * the mirror's same-frame effect shows (SPEC UI state). Navigate first: a view
 * switch subscribes through the same submit.
 */
async function parkSubmits(app: App): Promise<() => Promise<void>> {
  const parked: Route[] = [];
  let open = false;
  const submit = /\/invoke\/submit$/;
  await app.page.route(submit, (route) => (open ? route.continue() : parked.push(route)));
  return async () => {
    open = true;
    await each(parked, (r) => r.continue());
    await app.page.unroute(submit);
  };
}

/** Runs `action` on `row` with submits parked: the row leaves before Rust sees it, and stays out. */
async function leavesAtOnce(
  app: App,
  row: string,
  where: string,
  action: () => Promise<void>,
): Promise<void> {
  await highlight(app, row);
  const release = await parkSubmits(app);
  await action();
  await expect(app.row(row), `${row} leaves ${where} at once`).toHaveCount(0);
  await release();
  await app.settle();
  await expect(app.row(row), `${row} stays out of ${where}`).toHaveCount(0);
}

/** Selects the rows containing `first` and the `count - 1` below it (`x`, `Shift+J`). */
async function selectFrom(app: App, first: string, count: number): Promise<void> {
  await highlight(app, first);
  await app.press("x", ...Array.from({ length: count - 1 }, () => "Shift+J"));
  await expect(app.selected).toHaveCount(count);
}

// ------------------------------------------------------------------ cases

test("TRI-01 e on the list removes the row before Gmail answers, and moves the highlight", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  const hold = await gmail.hold("messages.batchModify");
  await app.press("j", "j");
  await expect(app.highlighted).toContainText("Person 3");
  await app.press("e");

  // Held: the row is gone and the former 4th row is 3rd, highlighted.
  await expect(app.row("Person 3")).toHaveCount(0);
  await expect(app.rows.nth(2)).toContainText("Person 4");
  await expect(app.highlighted).toContainText("Person 4");

  await hold.release();
  await app.settle();
  expect(await has(gmail, s, "p3", "INBOX")).toBe(false);
  const [req] = await gmail.requests("messages.batchModify");
  expect(req?.body).toMatchObject({ ids: s.thread("p3").messages.map((m) => m.id) });

  // The last row: the highlight moves to the new last row.
  await app.press(...Array.from({ length: 30 }, () => "j"));
  await expect(app.highlighted).toContainText("Your receipt");
  await app.press("e");
  await expect(app.row("Your receipt")).toHaveCount(0);
  await expect(app.highlighted).toContainText("Forum thread");
  await expect(app.rows.last()).toContainText("Forum thread");

  await go(app, "e");
  await expect(app.row("Person 3")).toHaveCount(1);
  await expect(app.row("Your receipt")).toHaveCount(1);
});

test("TRI-01 e on a long thread archives every one of its messages", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  const fold = s.thread("fold");
  const ids = fold.messages.map((m) => m.id);
  expect(ids).toHaveLength(11);
  await highlight(app, "Fold check");
  await app.press("e");
  await expect(app.row("Fold check")).toHaveCount(0);
  await app.settle();

  const [req] = await gmail.requests("messages.batchModify");
  expect(req?.body).toMatchObject({ ids, removeLabelIds: ["INBOX", "UNREAD"] });
  const msgs = threadMessages(await gmail.gmail(A), fold.threadId);
  expect(msgs).toHaveLength(11);
  expect(msgs.filter((m) => m.labels.includes("INBOX"))).toEqual([]);
});

test("TRI-02 e in an open conversation opens the next one, or the previous one, or returns to the empty list", async ({
  app,
  gmail,
}) => {
  await seeds.empty(gmail, app);
  const subjects = ["Xenon", "Yarrow", "Zephyr"];
  await each([...subjects.entries()], ([i, subject]) =>
    gmail.deliver({
      account: A,
      from: `${subject} Sender <${subject.toLowerCase()}@x.test>`,
      subject,
      text: `About ${subject}.`,
      date: dates.minutesAgo(i + 1),
    }),
  );
  await app.settle();
  expect(await app.subjects()).toEqual(subjects);

  await app.row("Yarrow").click();
  await expect(app.conversationTitle).toHaveText("Yarrow");
  await app.press("e");
  await expect(app.conversationTitle).toHaveText("Zephyr");
  await app.press("e");
  await expect(app.conversationTitle).toHaveText("Xenon");
  await app.press("e");
  await expect(app.page.getByText("You're all done.")).toBeVisible();
  await expect(app.rows).toHaveCount(0);

  await app.settle();
  const state = await gmail.gmail(A);
  expect(state.messages.filter((m) => m.labels.includes("INBOX"))).toEqual([]);
});

test("TRI-03 # moves to Trash and never touches draft messages", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  await highlight(app, "Person 4");
  await app.reply("r");
  await app.type("A reply I keep");
  await app.press("Escape", "Escape");
  await app.settle();
  const [draft] = (await gmail.gmail(A)).drafts;
  expect(draft?.message.threadId).toBe(s.thread("p4").threadId);
  // A poll stores Gmail's copy of the draft message in the local thread, so
  // the trash has a draft message it must leave out.
  await gmail.poll();
  await app.settle();
  await app.press("Escape");
  await expect(app.highlighted).toContainText("Person 4");

  const hold = await gmail.hold("messages.batchModify");
  await app.press("#");
  await expect(app.row("Person 4")).toHaveCount(0);
  await hold.release();
  await app.settle();

  const state = await gmail.gmail(A);
  const p4 = threadMessages(state, s.thread("p4").threadId);
  expect(p4.map((m) => m.id)).toEqual(s.thread("p4").messages.map((m) => m.id));
  for (const m of p4) {
    expect(m.labels).toContain("TRASH");
    expect(m.labels).not.toContain("INBOX");
  }
  // The Gmail draft is still there, and the trash never named its message.
  expect(state.drafts.map((d) => d.id)).toEqual([draft?.id]);
  const trashReq = (await gmail.requests("messages.batchModify")).find((r) =>
    JSON.stringify(r.body).includes("TRASH"),
  );
  expect(trashReq?.body).toMatchObject({ ids: s.thread("p4").messages.map((m) => m.id) });
  expect(JSON.stringify(trashReq?.body)).not.toContain(draft?.message.id ?? "missing");

  // From Starred.
  await go(app, "s");
  await highlight(app, "Trip ideas");
  await app.press("#");
  await expect(app.row("Trip ideas")).toHaveCount(0);

  // From Spam: SPAM goes, TRASH comes.
  await go(app, "!");
  await highlight(app, "Old spam");
  await app.press("#");
  await expect(app.row("Old spam")).toHaveCount(0);
  await app.settle();
  expect(await has(gmail, s, "spamOld", "SPAM")).toBe(false);
  expect(await has(gmail, s, "spamOld", "TRASH")).toBe(true);

  await go(app, "#");
  await expect(app.row("Person 4")).toHaveCount(1);
  await expect(app.row("Trip ideas")).toHaveCount(1);
});

test("TRI-04 ! moves to Spam", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  await highlight(app, "Person 5");
  await app.press("!");
  await expect(app.row("Person 5")).toHaveCount(0);
  await app.settle();
  expect(await has(gmail, s, "p5", "SPAM")).toBe(true);
  expect(await has(gmail, s, "p5", "INBOX")).toBe(false);
  await go(app, "!");
  await expect(app.row("Person 5")).toHaveCount(1);

  await go(app, "#");
  await highlight(app, "Old trash");
  await app.press("!");
  await expect(app.row("Old trash")).toHaveCount(0);
  await app.settle();
  expect(await has(gmail, s, "trashOld", "TRASH")).toBe(false);
  expect(await has(gmail, s, "trashOld", "SPAM")).toBe(true);
  await expect(app.row("Old trash")).toHaveCount(0);
});

test("TRI-05 star and read toggles", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  const own = () => app.row("Budget draft");

  // 1. Star: shown before Gmail answers.
  await highlight(app, "Budget draft");
  const hold = await gmail.hold("messages.batchModify");
  await app.press("s");
  await expect(own()).toHaveAttribute("data-starred", "true");
  await hold.release();
  await app.settle();
  expect(await has(gmail, s, "own", "STARRED")).toBe(true);
  await go(app, "s");
  await expect(app.row("Budget draft")).toHaveCount(1);

  // 2. Unstar.
  await go(app, "i");
  await highlight(app, "Budget draft");
  await app.press("s");
  await expect(own()).not.toHaveAttribute("data-starred");
  await app.settle();
  expect(await has(gmail, s, "own", "STARRED")).toBe(false);

  // 3. Unstar in Starred: the row leaves at once.
  await highlight(app, "Person 2");
  await app.press("s");
  await expect(app.row("Person 2")).toHaveAttribute("data-starred", "true");
  await go(app, "s");
  await highlight(app, "Person 2");
  const hold2 = await gmail.hold("messages.batchModify");
  await app.press("s");
  await expect(app.row("Person 2")).toHaveCount(0);
  await hold2.release();
  await app.settle();
  expect(await has(gmail, s, "p2", "STARRED")).toBe(false);

  // 4. Unread on the latest message only; read on every message.
  await go(app, "i");
  await highlight(app, "Budget draft");
  await expect(own()).not.toHaveAttribute("data-unread");
  await app.press("u");
  await expect(own()).toHaveAttribute("data-unread", "true");
  await app.settle();
  const unread = (await gmail.gmail(A)).messages
    .filter((m) => m.threadId === s.thread("own").threadId && m.labels.includes("UNREAD"))
    .map((m) => m.id);
  expect(unread).toEqual([s.latest("own").id]);

  await app.press("u");
  await expect(own()).not.toHaveAttribute("data-unread");
  await app.settle();
  const after = threadMessages(await gmail.gmail(A), s.thread("own").threadId);
  expect(after.filter((m) => m.labels.includes("UNREAD"))).toEqual([]);
});

test("TRI-06 x and Shift+J / Shift+K select; actions apply to the whole selection as one undo entry", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  const before = await app.subjects();
  await app.press("j", "x", "Shift+J", "Shift+J");
  await expect(app.selected).toHaveCount(3);
  await each([2, 3, 4], (n) =>
    expect(app.row(`Person ${n}`)).toHaveAttribute("data-selected", "true"),
  );
  await expect(app.row("Person 1")).not.toHaveAttribute("data-selected");

  await app.press("e");
  await each([2, 3, 4], (n) => expect(app.row(`Person ${n}`)).toHaveCount(0));
  await app.settle();
  expect(await hasEach(gmail, s, ["p2", "p3", "p4"], "INBOX")).toEqual([false, false, false]);

  // One z brings all three back where they were.
  await app.press("z");
  await expect.poll(() => app.subjects()).toEqual(before);
  await app.settle();
  expect(await hasEach(gmail, s, ["p2", "p3", "p4"], "INBOX")).toEqual([true, true, true]);

  // Shift+K extends upward.
  await highlight(app, "Person 4");
  await app.press("x", "Shift+K", "Shift+K");
  await expect(app.selected).toHaveCount(3);
  await each([2, 3, 4], (n) =>
    expect(app.row(`Person ${n}`)).toHaveAttribute("data-selected", "true"),
  );
  await expect(app.highlighted).toContainText("Person 2");
  await app.press("Escape");
  await expect(app.selected).toHaveCount(0);

  // With nothing selected, Shift+J extends from the highlighted row: both are selected.
  await highlight(app, "Person 2");
  await app.press("Shift+J");
  await expect(app.selected).toHaveCount(2);
  await each([2, 3], (n) =>
    expect(app.row(`Person ${n}`)).toHaveAttribute("data-selected", "true"),
  );
  await app.press("Escape");
  await expect(app.selected).toHaveCount(0);

  // Plain j replaces the selection with the row it lands on.
  await highlight(app, "Person 2");
  await app.press("x");
  await expect(app.selected).toHaveCount(1);
  await app.press("j");
  await expect(app.highlighted).toContainText("Person 3");
  await expect(app.row("Person 2")).not.toHaveAttribute("data-selected");
  await app.press("e");
  await expect(app.row("Person 3")).toHaveCount(0);
  await app.settle();
  await expect(app.row("Person 2")).toHaveCount(1);
  expect(await has(gmail, s, "p2", "INBOX")).toBe(true);
  expect(await has(gmail, s, "p3", "INBOX")).toBe(false);
});

test("TRI-07 s and u on a mixed selection follow the l rule", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  const keys = ["p1", "p2", "p3"];
  const people = ["Person 1", "Person 2", "Person 3"];
  const flags = async (attr: "data-starred" | "data-unread") =>
    Promise.all(people.map(async (p) => (await app.row(p).getAttribute(attr)) !== null));
  const inGmail = async (label: string) => {
    const state = await gmail.gmail(A);
    return keys.map((k) => threadHas(state, s.thread(k).threadId, label));
  };

  // 1 starred → all starred.
  await highlight(app, "Person 2");
  await app.press("s");
  await expect(app.row("Person 2")).toHaveAttribute("data-starred", "true");
  await selectFrom(app, "Person 1", 3);
  await app.press("s");
  await expect.poll(() => flags("data-starred")).toEqual([true, true, true]);
  await app.settle();
  expect(await inGmail("STARRED")).toEqual([true, true, true]);

  // All starred → all unstarred.
  await selectFrom(app, "Person 1", 3);
  await app.press("s");
  await expect.poll(() => flags("data-starred")).toEqual([false, false, false]);
  await app.settle();
  expect(await inGmail("STARRED")).toEqual([false, false, false]);

  // 1 unread (p3; p1 is marked read first) → all unread.
  await highlight(app, "Person 1");
  await app.press("u");
  await expect(app.row("Person 1")).not.toHaveAttribute("data-unread");
  await selectFrom(app, "Person 1", 3);
  expect(await flags("data-unread")).toEqual([false, false, true]);
  await app.press("u");
  await expect.poll(() => flags("data-unread")).toEqual([true, true, true]);
  await app.settle();
  expect(await inGmail("UNREAD")).toEqual([true, true, true]);

  // All unread → all read.
  await selectFrom(app, "Person 1", 3);
  await app.press("u");
  await expect.poll(() => flags("data-unread")).toEqual([false, false, false]);
  await app.settle();
  expect(await inGmail("UNREAD")).toEqual([false, false, false]);
});

test("TRI-08 Shift+E returns to Inbox from any view", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  // Setup: archive p1, snooze p2.
  await highlight(app, "Person 1");
  await app.press("e");
  await highlight(app, "Person 2");
  await snoozeHighlighted(app);
  await expect(app.row("Person 2")).toHaveCount(0);
  await app.settle();

  const cases: [view: string, row: string][] = [
    ["e", "Person 1"],
    ["#", "Old trash"],
    ["!", "Old spam"],
    ["h", "Person 2"],
  ];
  await each(cases, async ([view, row]) => {
    await go(app, view);
    await highlight(app, row);
    await app.press("Shift+E");
    await expect(app.row(row)).toHaveCount(0);
  });
  await app.settle();

  const state = await gmail.gmail(A);
  for (const k of ["p1", "trashOld", "spamOld", "p2"]) {
    expect(threadHas(state, s.thread(k).threadId, "INBOX"), k).toBe(true);
  }
  expect(threadHas(state, s.thread("trashOld").threadId, "TRASH")).toBe(false);
  expect(threadHas(state, s.thread("spamOld").threadId, "SPAM")).toBe(false);

  // The snooze is cleared: Snoozed is empty.
  await go(app, "h");
  await expect(app.page.getByText("Nothing here.")).toBeVisible();
  await go(app, "i");
  await each(cases, ([, row]) => expect(app.row(row)).toHaveCount(1));
});

test("TRI-09 Esc closes the open overlay before it clears a selection", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  const selectTwo = async () => {
    await highlight(app, "Person 1");
    await app.press("x", "Shift+J");
    await expect(app.selected).toHaveCount(2);
  };

  // The palette.
  await selectTwo();
  await app.press("Meta+k");
  await expect(app.palette).toBeVisible();
  await app.press("Escape");
  await expect(app.palette).toHaveCount(0);
  await expect(app.selected).toHaveCount(2);
  await app.press("Escape");
  await expect(app.selected).toHaveCount(0);

  // The snooze picker.
  await selectTwo();
  await app.press("h");
  await expect(app.palette).toBeVisible();
  await app.press("Escape");
  await expect(app.palette).toHaveCount(0);
  await expect(app.selected).toHaveCount(2);
  await app.press("Escape");
  await expect(app.selected).toHaveCount(0);

  // Nothing open.
  await selectTwo();
  await app.press("Escape");
  await expect(app.selected).toHaveCount(0);
});

test.describe("TRI-10 each action removes its row from the right lists", () => {
  // Every removal is checked with submits parked: Rust's own list patch would
  // remove the row too, so only a parked submit shows the mirror's table.
  test("TRI-10 e: leaves Inbox and Snoozed, stays in Starred", async ({ app, gmail }) => {
    await seeds.standard(gmail, app);
    await leavesAtOnce(app, "Person 1", "Inbox", () => app.press("e"));

    await highlight(app, "Person 3");
    await snoozeHighlighted(app);
    await go(app, "h");
    await leavesAtOnce(app, "Person 3", "Snoozed", () => app.press("e"));

    // Not in the table: the row stays, still starred, and keeps the highlight.
    await go(app, "i");
    await highlight(app, "Person 2");
    await app.press("s");
    await go(app, "s");
    await highlight(app, "Person 2");
    await app.press("e");
    await app.settle();
    await expect(app.row("Person 2")).toHaveAttribute("data-starred", "true");
    await expect(app.highlighted).toContainText("Person 2");
  });

  test("TRI-10 Shift+E: leaves Done, Snoozed, Trash, Spam", async ({ app, gmail }) => {
    await seeds.standard(gmail, app);
    await highlight(app, "Person 1");
    await app.press("e");
    await highlight(app, "Person 2");
    await snoozeHighlighted(app);
    const cases: [view: string, row: string][] = [
      ["e", "Person 1"],
      ["h", "Person 2"],
      ["#", "Old trash"],
      ["!", "Old spam"],
    ];
    await each(cases, async ([view, row]) => {
      await go(app, view);
      await leavesAtOnce(app, row, `g ${view}`, () => app.press("Shift+E"));
    });
  });

  test("TRI-10 h: leaves Inbox and Done", async ({ app, gmail }) => {
    await seeds.standard(gmail, app);
    await highlight(app, "Person 1");
    await app.press("e");
    await leavesAtOnce(app, "Person 2", "Inbox", () => snoozeHighlighted(app));

    await go(app, "e");
    await leavesAtOnce(app, "Person 1", "Done", () => snoozeHighlighted(app));
  });

  test("TRI-10 #: leaves Starred, Sent, a label view, Done", async ({ app, gmail }) => {
    await seeds.standard(gmail, app);
    await highlight(app, "Person 1");
    await app.press("e");

    await go(app, "s");
    await leavesAtOnce(app, "Trip ideas", "Starred", () => app.press("#"));

    await go(app, "t");
    await leavesAtOnce(app, "Sent 10 days ago", "Sent", () => app.press("#"));

    await goLabel(app, "Projects");
    await leavesAtOnce(app, "Project alpha", "Projects", () => app.press("#"));

    await go(app, "e");
    await leavesAtOnce(app, "Person 1", "Done", () => app.press("#"));
  });

  test("TRI-10 !: leaves Starred and Trash", async ({ app, gmail }) => {
    await seeds.standard(gmail, app);
    await go(app, "s");
    await leavesAtOnce(app, "Trip ideas", "Starred", () => app.press("!"));

    await go(app, "#");
    await leavesAtOnce(app, "Old trash", "Trash", () => app.press("!"));
  });

  test("TRI-10 unstar: leaves Starred", async ({ app, gmail }) => {
    await seeds.standard(gmail, app);
    await go(app, "s");
    await leavesAtOnce(app, "Trip ideas", "Starred", () => app.press("s"));
  });

  test("TRI-10 y, or l removing the current label: leaves the label view", async ({
    app,
    gmail,
  }) => {
    const s = await seeds.standard(gmail, app);
    await gmail.modify(A, s.message("p1", 1).id, [s.label(A, "Travel")], [], { poll: true });
    await app.settle();
    await goLabel(app, "Travel");
    await expect(app.row("Person 1")).toHaveCount(1);

    await leavesAtOnce(app, "Trip ideas", "Travel", () => app.press("y"));
    await leavesAtOnce(app, "Person 1", "Travel", async () => {
      await app.press("l");
      await app.palette.getByRole("option", { name: "✓ Travel" }).click();
    });
  });

  test("TRI-10 send or discard: leaves Drafts", async ({ app, gmail }) => {
    await seeds.drafts(gmail, app);
    await go(app, "d");
    await expect(app.row("Fwd: Contract")).toHaveCount(1);
    const drafts = await app.rows.count();

    // Send a new-message draft: the c screen opens and closes back onto Drafts.
    await highlight(app, "Idea");
    await app.press("Enter");
    await expect(app.page.getByRole("heading", { name: "New Message" })).toBeVisible();
    await app.settle();
    const releaseSend = await parkSubmits(app);
    await app.press("Meta+Enter");
    await expect(app.list).toBeVisible();
    await expect(app.rows).toHaveCount(drafts - 1);
    await expect(app.row("Idea")).toHaveCount(0);
    await releaseSend();
    await app.settle();
    await expect(app.row("Idea")).toHaveCount(0);

    // Discard a reply draft: its conversation opens with the composer.
    await app.row("Fwd: Contract").click();
    await expect(app.editor).toBeVisible();
    await app.settle();
    const releaseDiscard = await parkSubmits(app);
    await app.press("Meta+Shift+Comma");
    await expect(app.editor).toHaveCount(0);
    await app.press("Escape");
    await expect(app.list).toBeVisible();
    await expect(app.rows).toHaveCount(drafts - 2);
    await expect(app.row("Fwd: Contract")).toHaveCount(0);
    await releaseDiscard();
    await app.settle();
    await expect(app.row("Fwd: Contract")).toHaveCount(0);
  });
});

test("TRI-11 an action changes only the messages it recorded; a reply arriving meanwhile stays in Inbox", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  const m1 = s.message("p1", 1);
  const hold = await gmail.hold("messages.batchModify");
  await expect(app.highlighted).toContainText("Person 1");
  await app.press("e");
  await expect(app.row("Person 1")).toHaveCount(0);

  const m2 = await gmail.deliver({
    account: A,
    from: "Carol Chen <carol@corp.test>",
    subject: "Re: Person 1",
    text: "A reply while the archive waits.",
    threadId: m1.threadId,
    inReplyTo: m1.messageId,
    labels: ["INBOX", "UNREAD", "CATEGORY_PERSONAL"],
  });
  await app.settle();
  // The first try fails, so the archive is still queued once m2 is stored
  // locally: the retry must name only the message it recorded.
  await hold.release({ kind: "status", status: 503 });
  await app.settle();
  await gmail.clearFaults();
  await app.settle();

  const reqs = await gmail.requests("messages.batchModify");
  expect(reqs.map((r) => r.answer)).toEqual([503, 204]);
  for (const req of reqs) expect(req.body).toMatchObject({ ids: [m1.id] });
  const state = await gmail.gmail(A);
  expect(state.messages.find((m) => m.id === m1.id)?.labels).not.toContain("INBOX");
  expect(state.messages.find((m) => m.id === m2.id)?.labels).toContain("INBOX");

  const row = app.row("Person 1");
  await expect(row).toHaveCount(1);
  await expect(row).toHaveAttribute("data-unread", "true");
  await expect(row).toContainText("Carol Chen");
  await expect(row).toContainText("A reply while the archive waits.");
});

test("TRI-12 e marks the conversation read, and z makes the same messages unread again", async ({
  app,
  gmail,
}) => {
  await seeds.empty(gmail, app);
  const [first, second] = await gmail.deliverMany([
    { account: A, from: "Rae Read <rae@corp.test>", subject: "Half read", labels: ["INBOX"] },
    {
      account: A,
      from: "Una Unread <una@corp.test>",
      subject: "Re: Half read",
      labels: ["INBOX", "UNREAD"],
      after: 0,
    },
  ]);
  await app.settle();
  const labels = async (id: string | undefined) =>
    (await gmail.gmail(A)).messages.find((m) => m.id === id)?.labels ?? [];
  await expect(app.row("Half read")).toHaveAttribute("data-unread", "true");

  await app.press("e");
  await app.settle();
  expect(await labels(first?.id)).not.toContain("INBOX");
  expect(await labels(second?.id)).not.toContain("INBOX");
  expect(await labels(second?.id)).not.toContain("UNREAD");
  await app.press("g");
  await app.press("e");
  await expect(app.row("Half read")).not.toHaveAttribute("data-unread");

  await app.press("z");
  await app.settle();
  expect(await labels(first?.id)).toEqual(expect.arrayContaining(["INBOX"]));
  expect(await labels(first?.id)).not.toContain("UNREAD");
  expect(await labels(second?.id)).toEqual(expect.arrayContaining(["INBOX", "UNREAD"]));
  await app.press("g");
  await app.press("i");
  await expect(app.row("Half read")).toHaveAttribute("data-unread", "true");
});
