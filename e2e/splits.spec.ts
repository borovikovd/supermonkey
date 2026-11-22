import type { Locator } from "@playwright/test";

import type { App } from "./app.ts";
import { expect, test } from "./fixtures.ts";
import * as seeds from "./seeds.ts";
import { A, B } from "./seeds.ts";

const IMPORTANT = [
  "Q4 plan",
  "Fold check",
  "Offsite venue",
  "Ticket #42",
  "Guild meetup",
  "Budget draft",
  "Alias note",
  "Contract",
  "Links",
  "Latest rule",
  "Person 1",
  "Person 2",
  "Person 3",
  "Person 4",
  "Person 5",
  "Person 6",
];
const OTHER = [
  "Digest: week 44",
  "Digest: daily",
  "Digest: week 43",
  "Big sale",
  "New follower",
  "Forum thread",
  "Your receipt",
];
const NOTHING = "from:nobody@none.test";

async function splitOf(app: App, subject: string): Promise<string | null> {
  const sections = await app.sections();
  return sections.find((s) => s.subjects.includes(subject))?.name ?? null;
}

async function expectSplit(app: App, subject: string, split: string | null, why = "") {
  await expect
    .poll(() => splitOf(app, subject), { message: `${subject} lands in ${split} ${why}` })
    .toBe(split);
}

async function expectHeader(app: App, header: string) {
  await expect.poll(() => app.splitHeader()).toBe(header);
}

async function command(app: App, name: string): Promise<Locator> {
  await app.press("Meta+k");
  await expect(app.palette).toBeVisible();
  await app.palette.getByRole("option", { name: new RegExp(`^${name}`) }).click();
  return app.palette;
}

async function pin(app: App, how: string, choice: string): Promise<void> {
  await command(app, how);
  const picker = app.page.getByRole("dialog", { name: how });
  await expect(picker).toBeVisible();
  await picker.getByRole("option", { name: choice, exact: true }).click();
  await expect(picker).toHaveCount(0);
}

async function doneView(app: App): Promise<void> {
  await app.page.getByRole("navigation").getByRole("button", { name: /^Done/ }).click();
}

function splitSheet(app: App): Locator {
  return app.sheet(/^Split Inbox · /);
}

async function openSheet(app: App): Promise<Locator> {
  await command(app, "Split Inbox settings");
  await expect(splitSheet(app)).toBeVisible();
  return splitSheet(app);
}

const sheetRows = (sheet: Locator) => sheet.locator("div.border-b");

async function sheetNames(sheet: Locator): Promise<string[]> {
  return sheetRows(sheet).evaluateAll((rows) =>
    rows.map((r) => r.querySelector("input")?.value ?? ""),
  );
}

async function sheetRow(sheet: Locator, name: string): Promise<Locator> {
  const names = await sheetNames(sheet);
  const i = names.indexOf(name);
  if (i < 0) throw new Error(`no split ${name} in the sheet: ${names.join(", ")}`);
  return sheetRows(sheet).nth(i);
}

async function addSplit(sheet: Locator, name: string, query: string): Promise<void> {
  await sheet.getByRole("button", { name: "Add split", exact: true }).click();
  const row = await sheetRow(sheet, "New split");
  await row.locator("input").first().fill(name);
  await row.locator("input").nth(1).fill(query);
}

async function moveUp(sheet: Locator, name: string): Promise<void> {
  await (await sheetRow(sheet, name)).getByRole("button", { name: "▲" }).click();
}

async function toggle(sheet: Locator, name: string): Promise<void> {
  await (await sheetRow(sheet, name)).getByRole("switch").click();
}

async function save(app: App, sheet: Locator): Promise<void> {
  await sheet.getByRole("button", { name: "Save", exact: true }).click();
  await expect(sheet).toHaveCount(0);
  await app.settle();
}

async function editSplits(app: App, edit: (sheet: Locator) => Promise<void>): Promise<void> {
  const sheet = await openSheet(app);
  await edit(sheet);
  await save(app, sheet);
}

test("SPL-01 default routing to Important and Other, with counts in the header", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  const sections = await app.sections();
  expect(sections.map((s) => s.name)).toEqual(["Important", "Other"]);
  expect(sections[0]?.subjects.toSorted()).toEqual(IMPORTANT.toSorted());
  expect(sections[1]?.subjects.toSorted()).toEqual(OTHER.toSorted());
  const all = sections.flatMap((s) => s.subjects);
  expect(new Set(all).size).toBe(all.length);
  expect(await app.splitHeader()).toBe("Important 16 · Other 7");
  await expect(app.section("Important")).toHaveAccessibleName("Important 16");
  await expect(app.section("Other")).toHaveAccessibleName("Other 7");
  await expect(app.section("Important")).toContainText("Important");
  await expect(app.section("Important")).toContainText("16");

  await expect(app.activeSplit).toHaveText("Important");
  await app.press(...Array.from({ length: 15 }, () => "j"));
  await expect(app.highlighted).toContainText(sections[0]?.subjects.at(-1) ?? "?");
  await expect(app.activeSplit).toHaveText("Important");
  await app.press("j");
  await expect(app.highlighted).toContainText(sections[1]?.subjects[0] ?? "?");
  await expect(app.activeSplit).toHaveText("Other");
});

test("SPL-02 Tab and Shift+Tab move between visible splits", async ({ app, gmail }) => {
  await seeds.splits(gmail, app);
  await editSplits(app, (sheet) => addSplit(sheet, "VIP", "from:carol OR from:dave"));
  await expectHeader(app, "VIP 2 · Important 16 · Other 7");
  const sections = await app.sections();
  expect(sections.map((s) => s.name)).toEqual(["VIP", "Important", "Other"]);
  const [vip, important, other] = sections.map((s) => s.subjects[0] ?? "?");
  expect(sections[0]?.subjects).toEqual(["Q4 plan", "Offsite venue"]);

  await app.highlight("Offsite venue");
  await app.press("Tab");
  await expect(app.highlighted).toContainText(important ?? "?");
  await expect(app.section("Important")).toBeInViewport();
  await app.press("Tab");
  await expect(app.highlighted).toContainText(other ?? "?");
  await app.press("Tab");
  await app.settle();
  await expect(app.highlighted).toContainText(other ?? "?");

  await app.press("Shift+Tab");
  await expect(app.highlighted).toContainText(important ?? "?");
  await app.press("Shift+Tab");
  await expect(app.highlighted).toContainText(vip ?? "?");
  await app.press("Shift+Tab");
  await app.settle();
  await expect(app.highlighted).toContainText(vip ?? "?");

  await app.press("x", "Shift+J", "e");
  await expectHeader(app, "Important 16 · Other 7");
  await expect(app.highlighted).toContainText(important ?? "?");
  await app.press("Shift+Tab");
  await app.settle();
  await expect(app.highlighted).toContainText(important ?? "?");
  await app.press("Tab");
  await expect(app.highlighted).toContainText(other ?? "?");
  await app.press("Shift+Tab");
  await expect(app.highlighted).toContainText(important ?? "?");
});

test("SPL-03 pin precedence: sender over domain over label", async ({ app, gmail }) => {
  await seeds.splits(gmail, app);
  await editSplits(app, async (sheet) => {
    await addSplit(sheet, "VIP", NOTHING);
    await addSplit(sheet, "Team", NOTHING);
  });
  await expectSplit(app, "X marks", "Important");
  await expectSplit(app, "Y knot", "Important");
  const requests = (await gmail.requests()).length;
  const where = async (x: string, y: string, step: string) => {
    await expectSplit(app, "X marks", x, step);
    await expectSplit(app, "Y knot", y, step);
  };
  await app.highlight("X marks");

  await pin(app, "Add to split", "Label Proj → Team");
  await where("Team", "Important", "1: label Proj → Team");
  await pin(app, "Add to split", "Domain corp.test → VIP");
  await where("VIP", "VIP", "2: domain beats label");
  await pin(app, "Add to split", "Sender x@corp.test → Important");
  await where("Important", "VIP", "3: sender beats domain");
  await app.press("z");
  await where("VIP", "VIP", "4: z removes the sender pin");
  await app.press("z");
  await where("Team", "Important", "5: z removes the domain pin");
  await app.highlight("X marks");
  await pin(app, "Add to split", "Label Ops → VIP");
  await where("VIP", "Important", "6: VIP comes first in split order");

  await editSplits(app, (sheet) => moveUp(sheet, "Team"));
  await where("Team", "Important", "7: Team now comes first");

  await app.highlight("X marks");
  await pin(app, "Move to Important", "Sender x@corp.test");
  await expectSplit(app, "X marks", "Important", "8: pinned to Important");
  await editSplits(app, (sheet) => toggle(sheet, "Important"));
  await where("Team", "Other", "8: the pin's split is off");
  await editSplits(app, (sheet) => toggle(sheet, "Important"));
  await where("Important", "Important", "9: the pin was kept while its split was off");

  await app.settle();
  expect((await gmail.requests()).slice(requests)).toEqual([]);
});

test("SPL-04 pinning from the palette replaces an older pin; z restores it; a sender pin holds across syncs", async ({
  app,
  gmail,
}) => {
  test.slow();
  await seeds.standard(gmail, app);
  await app.highlight("Your receipt");
  await command(app, "Move to Important");
  const picker = app.page.getByRole("dialog", { name: "Move to Important" });
  await expect(picker.getByRole("option")).toHaveText([
    "Sender billing@shop.test",
    "Domain shop.test",
    "Label Receipts",
  ]);
  await picker.getByRole("option", { name: "Sender billing@shop.test", exact: true }).click();
  await expectSplit(app, "Your receipt", "Important", "after Move to Important");
  await pin(app, "Move to Other", "Sender billing@shop.test");
  await expectSplit(app, "Your receipt", "Other", "the new pin replaced the old one");
  await app.press("z");
  await expectSplit(app, "Your receipt", "Important", "z restores the replaced pin");
  await app.press("z");
  await expectSplit(app, "Your receipt", "Other", "z removes the pin");

  await app.highlight("Digest: week 44");
  await pin(app, "Move to Important", "Sender news@letters.test");
  await expectSplit(app, "Digest: week 44", "Important");
  await gmail.deliver({
    account: A,
    from: "Letters <news@letters.test>",
    to: ["subscribers@letters.test"],
    subject: "Digest: week 45",
    text: "Digest: week 45.",
    headers: [["List-Unsubscribe", "<mailto:leave@letters.test>"]],
    labels: ["INBOX", "UNREAD", "CATEGORY_UPDATES"],
    poll: false,
  });
  await app.pollAndSettle(A);
  await app.settle();
  await expectSplit(app, "Digest: week 45", "Important", "on arrival");
  await app.pollAndSettle(A);
  await app.settle();
  await expectSplit(app, "Digest: week 45", "Important", "after a poll");
  await gmail.expireHistory(A);
  await app.pollAndSettle(A);
  await app.settle();
  await expectSplit(app, "Digest: week 45", "Important", "after a full resync");
  await app.restart();
  await expectSplit(app, "Digest: week 45", "Important", "after a restart");
});

test("SPL-05 your own reply does not move a thread", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  await editSplits(app, (sheet) => addSplit(sheet, "Mine", "from:ana@acme.test"));
  await expectSplit(app, "Budget draft", "Important");
  await expectSplit(app, "Offsite venue", "Important");

  await app.highlight("Offsite venue");
  await app.press("Enter");
  await app.reply("r");
  await app.type("Booked the venue");
  await app.settle();
  await app.press("Meta+Enter");
  await app.advance(10_000);
  await app.settle();
  const sent = (await gmail.gmail(A)).messages.filter(
    (m) => m.labels.includes("SENT") && m.headers.subject === "Re: Offsite venue",
  );
  expect(sent, "the reply was sent").toHaveLength(1);
  await app.press("Escape");
  await expectSplit(app, "Offsite venue", "Important", "after A's reply");

  await gmail.deliver({
    account: A,
    from: `Ana Ames <${A}>`,
    to: [A],
    subject: "Note to self",
    text: "Remember the milk.",
    labels: ["INBOX", "SENT", "CATEGORY_PERSONAL"],
    poll: false,
  });
  await app.pollAndSettle(A);
  await app.settle();
  await expectSplit(app, "Note to self", "Mine");
});

test("SPL-06 custom splits: add, order, rename, turn off, delete", async ({ app, gmail }) => {
  test.slow();
  await seeds.standard(gmail, app);
  const requests = (await gmail.requests()).length;

  let sheet = await openSheet(app);
  await expect(sheet).toHaveAccessibleName(`Split Inbox · ${A}`);
  expect(await sheetNames(sheet)).toEqual(["Important", "Other"]);
  const important = await sheetRow(sheet, "Important");
  const other = await sheetRow(sheet, "Other");
  await expect(important.getByRole("switch")).toHaveAccessibleName("Turn Important on or off");
  await expect(important.getByRole("button", { name: "×" })).toHaveCount(0);
  await expect(other.getByRole("switch")).toHaveCount(0);
  await expect(other.getByRole("button", { name: "×" })).toHaveCount(0);

  await addSplit(sheet, "Receipts", "label:receipts");
  await save(app, sheet);
  await expectHeader(app, "Receipts 1 · Important 16 · Other 6");
  expect((await app.sections())[0]).toEqual({ name: "Receipts", subjects: ["Your receipt"] });

  await editSplits(app, (s) => addSplit(s, "Venue", "subject:venue OR subject:ticket"));
  await expectHeader(app, "Receipts 1 · Venue 2 · Important 14 · Other 6");
  await expectSplit(app, "Offsite venue", "Venue");
  await expectSplit(app, "Ticket #42", "Venue");

  await editSplits(app, async (s) => {
    await addSplit(s, "Carol", "from:carol");
    await moveUp(s, "Carol");
    expect(await sheetNames(s)).toEqual(["Receipts", "Carol", "Venue", "Important", "Other"]);
  });
  await expectSplit(app, "Offsite venue", "Carol");
  await expectHeader(app, "Receipts 1 · Carol 1 · Venue 1 · Important 14 · Other 6");
  await editSplits(app, (s) => moveUp(s, "Venue"));
  await expectSplit(app, "Offsite venue", "Venue");
  await expectHeader(app, "Receipts 1 · Venue 2 · Important 14 · Other 6");

  await editSplits(app, async (s) => {
    await (await sheetRow(s, "Venue")).locator("input").first().fill("Places");
  });
  await expectHeader(app, "Receipts 1 · Places 2 · Important 14 · Other 6");

  await editSplits(app, (s) => toggle(s, "Places"));
  await expectSplit(app, "Offsite venue", "Carol");
  await expectSplit(app, "Ticket #42", "Important");
  await expectHeader(app, "Receipts 1 · Carol 1 · Important 15 · Other 6");
  sheet = await openSheet(app);
  const places = await sheetRow(sheet, "Places");
  await expect(places.locator("input").nth(1)).toHaveValue("subject:venue OR subject:ticket");
  await expect(places.getByRole("switch")).toHaveAttribute("aria-checked", "false");

  await toggle(sheet, "Places");
  await save(app, sheet);
  await expectSplit(app, "Offsite venue", "Places");
  await expectSplit(app, "Ticket #42", "Places");

  await app.highlight("Your receipt");
  await pin(app, "Add to split", "Sender billing@shop.test → Receipts");
  await expectSplit(app, "Your receipt", "Receipts");
  const deleteReceipts = (s: Locator) =>
    sheetRow(s, "Receipts").then((row) => row.getByRole("button", { name: "×" }).click());
  await editSplits(app, deleteReceipts);
  await expectSplit(app, "Your receipt", "Other", "after deleting Receipts");
  await editSplits(app, (s) => addSplit(s, "Receipts", NOTHING));
  await expectSplit(app, "Your receipt", "Other", "a new Receipts does not bring the pin back");

  await app.settle();
  expect((await gmail.requests()).slice(requests), "saving makes no Gmail request").toEqual([]);

  await app.press("Control+2");
  sheet = await openSheet(app);
  await expect(sheet).toHaveAccessibleName(`Split Inbox · ${B}`);
  expect(await sheetNames(sheet)).toEqual(["Important", "Other"]);
});

test("SPL-07 starter splits, and a flat inbox", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  let sheet = await openSheet(app);
  await Promise.all(
    ["Newsletters", "Promotions", "Social", "Updates"].map((starter) =>
      expect(sheet.getByRole("button", { name: `+ ${starter}`, exact: true })).toBeVisible(),
    ),
  );
  expect(await sheetNames(sheet)).toEqual(["Important", "Other"]);
  await app.press("Escape");
  await expect(sheet).toHaveCount(0);
  expect(await app.splitHeader()).toBe("Important 16 · Other 7");

  sheet = await openSheet(app);
  await sheet.getByRole("button", { name: "+ Newsletters" }).click();
  await expect((await sheetRow(sheet, "Newsletters")).locator("input").nth(1)).toHaveValue(
    "is:list",
  );
  await save(app, sheet);
  await expectHeader(app, "Newsletters 3 · Important 16 · Other 4");
  await Promise.all(
    ["Digest: week 44", "Digest: daily", "Digest: week 43"].map((d) =>
      expectSplit(app, d, "Newsletters"),
    ),
  );

  sheet = await openSheet(app);
  await sheet.getByRole("button", { name: "+ Promotions" }).click();
  await expect((await sheetRow(sheet, "Promotions")).locator("input").nth(1)).toHaveValue(
    "label:CATEGORY_PROMOTIONS",
  );
  await save(app, sheet);
  await expectSplit(app, "Big sale", "Promotions");
  await expectSplit(app, "Your receipt", "Promotions");
  await expectHeader(app, "Newsletters 3 · Promotions 2 · Important 16 · Other 2");

  await editSplits(app, async (s) => {
    await toggle(s, "Newsletters");
    await toggle(s, "Promotions");
    await toggle(s, "Important");
  });
  await expectHeader(app, "Other 23");
  await expect(app.list.getByRole("heading")).toHaveCount(0);
  const sections = await app.sections();
  expect(sections).toHaveLength(1);
  expect(sections[0]?.subjects.toSorted()).toEqual([...IMPORTANT, ...OTHER].toSorted());
});

test("SPL-08 empty splits hide themselves, except after g i / g o", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  expect(await app.splitHeader()).toBe("Important 16 · Other 7");

  await expect(app.highlighted).toContainText("Person 1");
  await app.press("x", ...Array.from({ length: 15 }, () => "Shift+J"));
  await expect(app.selected).toHaveCount(16);
  await app.press("e");
  await app.settle();

  await expectHeader(app, "Other 7");
  await expect(app.section("Important")).toHaveCount(0);

  await editSplits(app, async (s) => {
    await (await sheetRow(s, "Important")).getByRole("checkbox").uncheck();
  });
  await expectHeader(app, "Important · Other 7");
  await expect(app.section("Important")).toHaveCount(0);

  await editSplits(app, async (s) => {
    await (await sheetRow(s, "Important")).getByRole("checkbox").check();
  });
  await expectHeader(app, "Other 7");

  await doneView(app);
  await expect(app.page.getByRole("toolbar", { name: "Splits" })).toHaveCount(0);
  await app.press("g", "i");
  await expect(app.section("Important")).toBeVisible();
  await expect
    .poll(async () => (await app.sections())[0])
    .toEqual({
      name: "Important",
      subjects: [],
    });
  await app.press("g", "o");
  await expect(app.highlighted).toContainText("Digest: week 44");

  await editSplits(app, (s) => toggle(s, "Important"));
  await doneView(app);
  await app.press("g", "i");
  await expect(app.row("Digest: week 44")).toHaveCount(1);
  await app.settle();
  await expect(app.section("Important")).toHaveCount(0);
  expect(await app.splitHeader()).toBe("Other 7");
});

test("SPL-09 counts cap at 999+", async ({ app, gmail }) => {
  test.slow();
  await seeds.bulk1000(gmail, app);
  await expect(app.splitTitles().first()).toHaveText("Important");
  await expect.poll(() => app.splitHeader()).toMatch(/^Important 999( ·|$)/);
  await gmail.deliver({
    account: A,
    from: "Person 1000 <p1000@many.test>",
    to: [A],
    subject: "Thread 1000",
    text: "Mail 1000.",
    labels: ["INBOX", "CATEGORY_PERSONAL"],
    poll: false,
  });
  await app.pollAndSettle(A);
  await app.settle();
  await expect.poll(() => app.splitHeader()).toMatch(/^Important 999\+( ·|$)/);
  expect(await app.splitHeader()).toBe("Important 999+");
});

test("SPL-10 clicking a split name scrolls to its section", async ({ app, gmail }) => {
  await seeds.bulk(gmail, app);
  expect(await app.splitHeader()).toBe("Important 40 · Other 20");
  await app.splitTitles().filter({ hasText: "Other" }).click();
  await expect
    .poll(() =>
      app.list.evaluate((list) => {
        const viewport = list.parentElement?.getBoundingClientRect().top ?? Number.NaN;
        const header = list.querySelector("h2[aria-label^='Other']")?.getBoundingClientRect().top;
        return header === undefined ? "not rendered" : Math.round(header - viewport);
      }),
    )
    .toBe(0);
  await expect(app.activeSplit).toHaveText("Other");
});

test("SPL-11 a rule the core refuses keeps the sheet open with every edit", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  const sheet = await openSheet(app);
  await addSplit(sheet, "Receipts", "label:receipts");
  await toggle(sheet, "Important");
  await addSplit(sheet, "Carol", "from:carol -venue");
  const rule = (await sheetRow(sheet, "Carol")).locator("input").nth(1);

  await sheet.getByRole("button", { name: "Save", exact: true }).click();
  await app.settle();
  await expect(sheet).toBeVisible();
  await expect(sheet.getByRole("alert")).toHaveText("-venue isn't supported in split rules");
  await expect(rule).toHaveAttribute("aria-invalid", "true");
  await expect(rule).toHaveAccessibleDescription("-venue isn't supported in split rules");
  await app.expectNoErrorToasts();
  expect(await sheetNames(sheet)).toEqual(["Receipts", "Carol", "Important", "Other"]);
  await expect(rule).toHaveValue("from:carol -venue");
  await expect((await sheetRow(sheet, "Important")).getByRole("switch")).toHaveAttribute(
    "aria-checked",
    "false",
  );
  await expectHeader(app, "Important 16 · Other 7");

  await rule.fill("from:carol");
  await expect(sheet.getByRole("alert")).toHaveCount(0);
  await save(app, sheet);
  await expectSplit(app, "Offsite venue", "Carol");
  await expectSplit(app, "Your receipt", "Receipts");
  await expectSplit(app, "Person 1", "Other", "with Important off");
});
