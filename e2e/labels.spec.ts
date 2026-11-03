// Labels (COVERAGE 5.7, LAB): `l` add and remove, selections, creating a label
// from the picker (online, offline, and when Gmail already has it), `y`, and
// labels first seen in history.
import type { App } from "./app.ts";
import type { Control, GmailState } from "./control.ts";
import { expect, test } from "./fixtures.ts";
import * as seeds from "./seeds.ts";
import { A, type Seeded } from "./seeds.ts";

// ------------------------------------------------------------------ helpers

/**
 * Whether Gmail puts the thread in a label's view (COVERAGE 1): TRASH and SPAM
 * on any message; anything else on a message outside trash and spam.
 */
function threadHas(state: GmailState, threadId: string, label: string): boolean {
  const msgs = state.messages.filter((m) => m.threadId === threadId && !m.labels.includes("DRAFT"));
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

/** Gmail labels with this exact name. */
async function labelsNamed(gmail: Control, name: string) {
  return (await gmail.gmail(A)).labels.filter((l) => l.name === name);
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

/** Selects the rows containing `first` and the `count - 1` below it (`x`, `Shift+J`). */
async function selectFrom(app: App, first: string, count: number): Promise<void> {
  await highlight(app, first);
  await app.press("x", ...Array.from({ length: count - 1 }, () => "Shift+J"));
  await expect(app.selected).toHaveCount(count);
}

/** `g` `l`, then picks a user label by name. */
async function goLabel(app: App, name: string): Promise<void> {
  await app.press("g", "l");
  await expect(app.palette).toBeVisible();
  await app.palette.getByRole("option", { name, exact: true }).click();
  await expect(app.palette).toHaveCount(0);
}

/** The `l` picker's options. */
const picker = (app: App) => app.palette.getByRole("option");

/** Opens the `l` picker and picks `name` (with or without its ✓). */
async function pickLabel(app: App, name: string): Promise<void> {
  await app.press("l");
  await expect(app.palette).toBeVisible();
  await picker(app)
    .filter({ hasText: new RegExp(`^(✓ )?${name}$`) })
    .click();
  await expect(app.palette).toHaveCount(0);
}

/** Opens the `l` picker, types a new name, and chooses `Create label "<name>"`. */
async function createLabel(app: App, name: string): Promise<void> {
  await app.press("l");
  await expect(app.palette).toBeVisible();
  await app.type(name);
  await picker(app)
    .filter({ hasText: `Create label "${name}"` })
    .click();
  await expect(app.palette).toHaveCount(0);
}

// ------------------------------------------------------------------ cases

test("LAB-01 l adds and removes a user label", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  const travel = s.label(A, "Travel");
  await expect(app.highlighted).toContainText("Person 1");

  // Only user labels.
  await app.press("l");
  await expect(app.palette).toBeVisible();
  await expect(app.palette).toHaveAccessibleName("Label as…");
  await expect
    .poll(async () => (await picker(app).allInnerTexts()).toSorted())
    .toEqual(["Projects", "Receipts", "Travel", "Work/Clients"]);

  // Add, before Gmail answers.
  const hold = await gmail.hold("messages.batchModify");
  await app.type("Tra");
  await expect(picker(app).first()).toHaveText("Travel");
  await app.press("Enter");
  await expect(app.palette).toHaveCount(0);
  await goLabel(app, "Travel");
  await expect(app.row("Person 1")).toHaveCount(1);
  await hold.release();
  await app.settle();
  expect(await has(gmail, s, "p1", travel)).toBe(true);

  // Remove: the picker shows the ✓.
  await highlight(app, "Person 1");
  await app.press("l");
  await expect(picker(app).filter({ hasText: "Travel" })).toHaveText("✓ Travel");
  await picker(app).filter({ hasText: "Travel" }).click();
  await app.settle();
  const p1 = (await gmail.gmail(A)).messages.filter((m) => m.threadId === s.thread("p1").threadId);
  expect(p1.length).toBeGreaterThan(0);
  for (const m of p1) expect(m.labels).not.toContain(travel);
});

test("LAB-02 l on a selection: remove only if every thread has the label", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  const travel = s.label(A, "Travel");

  // All three have Travel: it is removed from all.
  await each(["p1", "p2", "p3"], (k) => gmail.modify(A, s.message(k, 1).id, [travel], []));
  await gmail.poll();
  await app.settle();
  await selectFrom(app, "Person 1", 3);
  await app.press("l");
  await expect(picker(app).filter({ hasText: "Travel" })).toHaveText("✓ Travel");
  await picker(app).filter({ hasText: "Travel" }).click();
  await app.settle();
  expect(await hasEach(gmail, s, ["p1", "p2", "p3"], travel)).toEqual([false, false, false]);

  // One of three has Travel: it is added to all.
  await gmail.modify(A, s.message("p4", 1).id, [travel], [], { poll: true });
  await app.settle();
  await selectFrom(app, "Person 4", 3);
  await app.press("l");
  await expect(picker(app).filter({ hasText: "Travel" })).toHaveText("Travel");
  await picker(app).filter({ hasText: "Travel" }).click();
  await app.settle();
  expect(await hasEach(gmail, s, ["p4", "p5", "p6"], travel)).toEqual([true, true, true]);

  // One z restores each thread's own earlier state.
  await app.press("z");
  await app.settle();
  expect(await hasEach(gmail, s, ["p4", "p5", "p6"], travel)).toEqual([true, false, false]);
});

test("LAB-03 create a label from the picker and apply it in one action", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  await selectFrom(app, "Person 1", 2);
  const hold = await gmail.hold("labels.create");
  await createLabel(app, "Invoices 2026");

  // At once: the label view lists both.
  await goLabel(app, "Invoices 2026");
  await expect(app.row("Person 1")).toHaveCount(1);
  await expect(app.row("Person 2")).toHaveCount(1);
  await hold.release();
  await app.settle();

  const named = await labelsNamed(gmail, "Invoices 2026");
  expect(named).toHaveLength(1);
  const id = named[0]?.id ?? "missing";
  expect(await hasEach(gmail, s, ["p1", "p2"], id)).toEqual([true, true]);
  await app.press("g", "l");
  await expect(app.palette.getByRole("option", { name: "Invoices 2026", exact: true })).toHaveCount(
    1,
  );
  await app.press("Escape");
  await expect(app.palette).toHaveCount(0);

  await app.press("z");
  await app.settle();
  expect(await hasEach(gmail, s, ["p1", "p2"], id)).toEqual([false, false]);
});

test("LAB-04 a label created offline is mapped to its Gmail id when the queue replays", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  await gmail.offline(true);
  await highlight(app, "Person 3");
  await createLabel(app, "Offline L");
  await highlight(app, "Person 4");
  await pickLabel(app, "Travel");

  await goLabel(app, "Offline L");
  await expect(app.row("Person 3")).toHaveCount(1);

  await gmail.offline(false);
  await app.settle();
  const named = await labelsNamed(gmail, "Offline L");
  expect(named).toHaveLength(1);
  const id = named[0]?.id ?? "missing";
  expect(await has(gmail, s, "p3", id)).toBe(true);
  expect(await has(gmail, s, "p4", s.label(A, "Travel"))).toBe(true);

  const log = await gmail.requests();
  const create = log.find((r) => r.api === "labels.create" && r.answer === 200);
  const apply = log.find(
    (r) =>
      r.api === "messages.batchModify" &&
      r.answer === 204 &&
      JSON.stringify(r.body).includes(`"${id}"`),
  );
  expect(create).toBeDefined();
  expect(apply).toBeDefined();
  expect(create?.seq ?? Infinity).toBeLessThan(apply?.seq ?? -Infinity);

  await expect(app.row("Person 3")).toHaveCount(1);
});

test("LAB-05 if labels.create says the name exists, the existing label is used", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  // Made by another client; the app has not refreshed its labels.
  const { id } = await gmail.label(A, "Clients");
  await highlight(app, "Person 5");
  await createLabel(app, "Clients");
  await app.settle();

  await expect(app.errorToasts).toHaveCount(0);
  expect(await labelsNamed(gmail, "Clients")).toEqual([{ id, name: "Clients" }]);
  expect(await has(gmail, s, "p5", id)).toBe(true);

  await app.press("l");
  await expect(app.palette).toBeVisible();
  await expect(picker(app).filter({ hasText: /^(✓ )?Clients$/ })).toHaveCount(1);
});

test("LAB-06 y removes the current view's label: label view, Inbox, Starred, Trash, Spam", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  const cases: [view: () => Promise<void>, row: string, key: string, label: string][] = [
    [() => goLabel(app, "Travel"), "Trip ideas", "starredOld", s.label(A, "Travel")],
    [() => app.press("g", "i"), "Person 1", "p1", "INBOX"],
    [() => app.press("g", "s"), "Trip ideas", "starredOld", "STARRED"],
    [() => app.press("g", "#"), "Old trash", "trashOld", "TRASH"],
    [() => app.press("g", "!"), "Old spam", "spamOld", "SPAM"],
  ];
  await each(cases, async ([open, row, key, label]) => {
    await open();
    await highlight(app, row);
    await app.press("y");
    await expect(app.row(row), `${row} leaves`).toHaveCount(0);
    await app.settle();
    expect(await has(gmail, s, key, label), `${key} loses ${label}`).toBe(false);
  });
});

test("LAB-06 y does nothing in Sent, Drafts, Done, Snoozed, and Search", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  // Setup: p1 in Done, p2 snoozed, a reply draft on p3.
  await expect(app.highlighted).toContainText("Person 1");
  await app.press("e");
  await highlight(app, "Person 2");
  await app.press("h");
  await expect(app.palette.getByRole("option").first()).toBeVisible();
  await app.press("Enter");
  await expect(app.row("Person 2")).toHaveCount(0);
  await highlight(app, "Person 3");
  await app.reply("r");
  await app.type("Kept as a draft");
  await app.press("Escape", "Escape", "Escape");
  await expect(app.list).toBeVisible();
  await app.settle();
  const before = (await gmail.requests("messages.batchModify")).length;

  const cases: [view: string, row: string][] = [
    ["t", "Sent 10 days ago"],
    ["d", "Kept as a draft"],
    ["e", "Person 1"],
    ["h", "Person 2"],
  ];
  await each(cases, async ([view, row]) => {
    await app.press("g", view);
    await highlight(app, row);
    const rows = await app.rows.count();
    await app.press("y");
    await app.settle();
    await expect(app.row(row), `${row} stays in g ${view}`).toHaveCount(1);
    await expect(app.rows).toHaveCount(rows);
  });

  // Search results.
  await app.press("/");
  await app.type("Person 4");
  await app.press("Enter");
  await expect(app.row("Person 4")).toHaveCount(1);
  await expect(app.highlighted).toContainText("Person 4");
  await app.press("y");
  await app.settle();
  await expect(app.row("Person 4")).toHaveCount(1);

  expect(await gmail.requests("messages.batchModify")).toHaveLength(before);
});

test("LAB-07 a label first seen in history is picked up", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  const { id } = await gmail.label(A, "Legal");
  await gmail.modify(A, s.message("p2", 1).id, [id], [], { poll: true });
  await app.settle();

  await goLabel(app, "Legal");
  await expect(app.row("Person 2")).toHaveCount(1);
  await highlight(app, "Person 2");
  await app.press("l");
  await expect(app.palette).toBeVisible();
  await expect(picker(app).filter({ hasText: "Legal" })).toHaveText("✓ Legal");
});

test("LAB-08 a label list read before a create landed does not delete the new label", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  // A focus refresh reads the labels now; its answer comes late.
  const list = await gmail.holdAnswer("labels.list", { times: 1 });
  await app.focus(false);
  await app.focus(true);
  await expect
    .poll(async () => (await gmail.requests("labels.list")).some((r) => r.held))
    .toBe(true);
  await highlight(app, "Person 1");
  await createLabel(app, "Fresh L");
  await app.settle();
  const id = (await labelsNamed(gmail, "Fresh L"))[0]?.id ?? "missing";

  await list.release();
  await app.settle();
  await app.press("g", "l");
  await expect(
    app.palette.getByRole("option", { name: "Fresh L", exact: true }),
    "the new label is kept",
  ).toHaveCount(1);
  await app.press("Escape");
  await expect(app.palette).toHaveCount(0);
  await goLabel(app, "Fresh L");
  await expect(app.row("Person 1")).toHaveCount(1);
  await app.press("g", "i");
  await highlight(app, "Person 2");
  await pickLabel(app, "Fresh L");
  await app.settle();
  expect(await has(gmail, s, "p2", id)).toBe(true);
  expect(await labelsNamed(gmail, "Fresh L")).toHaveLength(1);
});
