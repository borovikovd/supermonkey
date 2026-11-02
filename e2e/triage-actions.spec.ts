// Triage actions (COVERAGE 5.5, TRI): archive, return to inbox, trash, spam,
// star, read, selection, the removal table, and the ids an action records.
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
  await app.settle();
  await expect(app.row("Old trash")).toHaveCount(0);
});

test("TRI-05 star and read toggles", async ({ app, gmail }) => {
  test.fail(
    true,
    "BUG: u marks only the latest message unread locally, but the batchModify adds UNREAD to every message id of the thread (src-tauri/src/mail.rs:134,169)",
  );
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
  // Every case holds batchModify, so only the app's own removal is observed.
  test("TRI-10 e: leaves Snoozed, stays in Starred", async ({ app, gmail }) => {
    await seeds.standard(gmail, app);
    // Not in the table: the row stays, and the thread is still starred.
    await highlight(app, "Person 2");
    await app.press("s");
    await go(app, "s");
    await highlight(app, "Person 2");
    await app.press("e");
    await app.settle();
    await expect(app.row("Person 2")).toHaveCount(1);
    await expect(app.row("Person 2")).toHaveAttribute("data-starred", "true");

    await gmail.hold("messages.batchModify");
    await go(app, "i");
    await highlight(app, "Person 1");
    await snoozeHighlighted(app);
    await go(app, "h");
    await highlight(app, "Person 1");
    await app.press("e");
    await expect(app.row("Person 1")).toHaveCount(0);
  });

  test("TRI-10 Shift+E: leaves Done, Snoozed, Trash, Spam", async ({ app, gmail }) => {
    await seeds.standard(gmail, app);
    await gmail.hold("messages.batchModify");
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
      await highlight(app, row);
      await app.press("Shift+E");
      await expect(app.row(row), `${row} in g ${view}`).toHaveCount(0);
    });
  });

  test("TRI-10 h: leaves Inbox and Done", async ({ app, gmail }) => {
    await seeds.standard(gmail, app);
    await gmail.hold("messages.batchModify");
    await highlight(app, "Person 1");
    await app.press("e");
    await highlight(app, "Person 2");
    await snoozeHighlighted(app);
    await expect(app.row("Person 2")).toHaveCount(0);

    await go(app, "e");
    await highlight(app, "Person 1");
    await snoozeHighlighted(app);
    await expect(app.row("Person 1")).toHaveCount(0);
  });

  test("TRI-10 #: leaves Starred, Sent, a label view, Done", async ({ app, gmail }) => {
    await seeds.standard(gmail, app);
    await gmail.hold("messages.batchModify");
    await highlight(app, "Person 1");
    await app.press("e");

    await go(app, "s");
    await highlight(app, "Trip ideas");
    await app.press("#");
    await expect(app.row("Trip ideas")).toHaveCount(0);

    await go(app, "t");
    await highlight(app, "Sent 10 days ago");
    await app.press("#");
    await expect(app.row("Sent 10 days ago")).toHaveCount(0);

    await goLabel(app, "Projects");
    await highlight(app, "Project alpha");
    await app.press("#");
    await expect(app.row("Project alpha")).toHaveCount(0);

    await go(app, "e");
    await highlight(app, "Person 1");
    await app.press("#");
    await expect(app.row("Person 1")).toHaveCount(0);
  });

  test("TRI-10 !: leaves Starred and Trash", async ({ app, gmail }) => {
    await seeds.standard(gmail, app);
    await gmail.hold("messages.batchModify");
    await go(app, "s");
    await highlight(app, "Trip ideas");
    await app.press("!");
    await expect(app.row("Trip ideas")).toHaveCount(0);

    await go(app, "#");
    await highlight(app, "Old trash");
    await app.press("!");
    await expect(app.row("Old trash")).toHaveCount(0);
  });

  test("TRI-10 unstar: leaves Starred", async ({ app, gmail }) => {
    await seeds.standard(gmail, app);
    await gmail.hold("messages.batchModify");
    await go(app, "s");
    await highlight(app, "Trip ideas");
    await app.press("s");
    await expect(app.row("Trip ideas")).toHaveCount(0);
  });

  test("TRI-10 y, or l removing the current label: leaves the label view", async ({
    app,
    gmail,
  }) => {
    const s = await seeds.standard(gmail, app);
    await gmail.modify(A, s.message("p1", 1).id, [s.label(A, "Travel")], [], { poll: true });
    await app.settle();
    await gmail.hold("messages.batchModify");
    await goLabel(app, "Travel");
    await expect(app.row("Person 1")).toHaveCount(1);

    await highlight(app, "Trip ideas");
    await app.press("y");
    await expect(app.row("Trip ideas")).toHaveCount(0);

    await highlight(app, "Person 1");
    await app.press("l");
    await app.palette.getByRole("option", { name: "✓ Travel" }).click();
    await expect(app.row("Person 1")).toHaveCount(0);
  });

  test("TRI-10 send or discard: leaves Drafts", async ({ app, gmail }) => {
    await seeds.drafts(gmail, app);
    await go(app, "d");

    // Send a new-message draft: the c screen opens and closes back onto Drafts.
    await highlight(app, "Idea");
    await app.press("Enter");
    await expect(app.page.getByRole("heading", { name: "New Message" })).toBeVisible();
    await app.settle();
    await app.press("Meta+Enter");
    await expect(app.list).toBeVisible();
    await expect(app.row("Idea")).toHaveCount(0);

    // Discard a reply draft: its conversation opens with the composer.
    await app.row("Fwd: Contract").click();
    await expect(app.editor).toBeVisible();
    await app.settle();
    await app.press("Meta+Shift+Comma");
    await expect(app.editor).toHaveCount(0);
    await app.press("Escape");
    await expect(app.list).toBeVisible();
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
  await hold.release();
  await app.settle();

  const [req] = await gmail.requests("messages.batchModify");
  expect(req?.body).toMatchObject({ ids: [m1.id] });
  const state = await gmail.gmail(A);
  expect(state.messages.find((m) => m.id === m1.id)?.labels).not.toContain("INBOX");
  expect(state.messages.find((m) => m.id === m2.id)?.labels).toContain("INBOX");

  const row = app.row("Person 1");
  await expect(row).toHaveCount(1);
  await expect(row).toHaveAttribute("data-unread", "true");
  await expect(row).toContainText("Carol Chen");
  await expect(row).toContainText("A reply while the archive waits.");
});
