// Search (COVERAGE 5.8, SPEC §Search): local results at once, the Gmail
// merge, the 300 ms debounce, offline search, the query language, actions on
// results, and paging.
import type { App } from "./app.ts";
import type { Control } from "./control.ts";
import { expect, test } from "./fixtures.ts";
import * as seeds from "./seeds.ts";
import { A, dates } from "./seeds.ts";

/** Runs `step` on each item, one after another. */
async function inOrder<T>(items: readonly T[], step: (item: T) => Promise<void>): Promise<void> {
  await items.reduce((prev, item) => prev.then(() => step(item)), Promise.resolve());
}

function field(app: App) {
  return app.page.getByPlaceholder("Search");
}

/** Opens search if needed and replaces the query, as typing would. */
async function search(app: App, query: string): Promise<void> {
  await app.press("/");
  await expect(field(app)).toBeFocused();
  await field(app).fill(query);
  await app.settle();
}

/** Moves the list highlight onto the row with this subject with `j` / `k`. */
async function highlightRow(app: App, subject: string): Promise<void> {
  await expect(app.row(subject)).toHaveCount(1);
  const subjects = await app.subjects();
  const to = subjects.indexOf(subject);
  if (to < 0) throw new Error(`no row ${subject} in ${subjects.join(", ")}`);
  const keys = await app.rows.evaluateAll((els) => els.map((el) => el.getAttribute("data-row")));
  const from = keys.indexOf(await app.highlighted.getAttribute("data-row"));
  const key = to > from ? "j" : "k";
  await app.press(...Array.from({ length: Math.abs(to - Math.max(from, 0)) }, () => key));
  await expect(app.highlighted).toContainText(subject);
}

/** The request log's length now: a mark for `gmailSearches`. */
async function mark(gmail: Control): Promise<number> {
  return (await gmail.requests()).length;
}

/** The `q` of each Gmail search since the mark (the contacts backfill also lists threads). */
async function gmailSearches(gmail: Control, since: number): Promise<string[]> {
  return (await gmail.requests())
    .slice(since)
    .filter((r) => r.api === "threads.list" && r.account === A)
    .map((r) => r.params.q)
    .filter((q): q is string => q !== undefined);
}

/**
 * Scrolls the list from top to bottom and returns every row it rendered, as
 * key → subject.
 */
async function collectRows(app: App): Promise<Map<string, string>> {
  const seen = new Map<string, string>();
  const scroller = app.list.locator("xpath=..");
  await scroller.evaluate((el) => {
    el.scrollTop = 0;
  });
  const frame = () =>
    app.page.evaluate(
      () => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))),
    );
  const step = async (left: number): Promise<void> => {
    await frame();
    const rows = await app.rows.evaluateAll((els) =>
      els.map((el) => [
        el.getAttribute("data-row") ?? "",
        el.querySelector(".flex-1 > .truncate")?.firstChild?.textContent ?? "",
      ]),
    );
    for (const [key, subject] of rows) if (key) seen.set(key, subject ?? "");
    const moved = await scroller.evaluate((el) => {
      const before = el.scrollTop;
      el.scrollTop += el.clientHeight;
      return el.scrollTop !== before;
    });
    if (moved && left > 0) await step(left - 1);
  };
  await step(50);
  return seen;
}

/** The rows' subjects once all of them are loaded, sorted. */
async function allSubjects(app: App): Promise<string[]> {
  const rows = await collectRows(app);
  return [...rows.values()].toSorted();
}

const forty = (prefix: string) =>
  Array.from({ length: 40 }, (_, i) => `${prefix} ${String(i + 1).padStart(2, "0")}`).toSorted();

test("SRC-01 / shows local results at once; focus moves to results; Esc restores the previous view", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  // Star p1 and p2 so Starred has two rows, then hold Gmail search forever.
  await app.press("s", "j", "s");
  await app.settle();
  await gmail.hold("threads.list");
  await app.page
    .getByRole("navigation")
    .getByRole("button", { name: /^Starred/ })
    .click();
  await expect(app.row("Person 2")).toHaveCount(1);
  await highlightRow(app, "Person 2");
  const starredKey = await app.rowKey("Person 2");
  const writes = (await gmail.requests("messages.batchModify")).length;

  const backOnStarred = async () => {
    await expect(field(app)).toHaveCount(0);
    await expect(app.page.getByText("Starred", { exact: true })).toBeVisible();
    await expect(app.highlighted).toHaveAttribute("data-row", starredKey);
  };

  await app.press("/");
  await expect(field(app)).toBeFocused();
  // Every key goes into the field, even shortcut keys.
  await app.type("venue e#j!");
  await expect(field(app)).toHaveValue("venue e#j!");
  await app.settle();
  expect(await gmail.requests("messages.batchModify")).toHaveLength(writes);

  await app.press("Meta+a", "Backspace");
  await app.type("venue");
  // Local results, without waiting for Gmail.
  await expect(app.row("Offsite venue")).toHaveCount(1);
  await app.press("ArrowDown");
  await expect(field(app)).not.toBeFocused();
  await app.press("j", "k", "s");
  await expect(app.row("Offsite venue")).toHaveAttribute("data-starred", "true");
  await app.press("Escape");
  await backOnStarred();

  // Enter instead of ↓.
  await app.press("/");
  await app.type("venue");
  await expect(app.row("Offsite venue")).toHaveCount(1);
  await app.press("Enter");
  await expect(field(app)).not.toBeFocused();
  await app.press("s");
  await expect(app.row("Offsite venue")).not.toHaveAttribute("data-starred");
  await app.press("Escape");
  await backOnStarred();

  // Esc while focus is still in the field.
  await app.press("/");
  await app.type("venue");
  await expect(app.row("Offsite venue")).toHaveCount(1);
  await app.press("Escape");
  await backOnStarred();
});

test("SRC-02 local query language, and what is sent to Gmail", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  // Cache trashOld through the Trash view, so its exclusion is not vacuous.
  await app.press("Meta+k");
  await app.palette.getByRole("option", { name: /^Go to Trash/ }).click();
  await expect(app.row("Old trash")).toHaveCount(1);
  await app.page
    .getByRole("navigation")
    .getByRole("button", { name: "Inbox", exact: true })
    .click();
  const since = await mark(gmail);

  const table: [string, string[], string[]][] = [
    ["quokka", ["Person 3"], []],
    ["from:carol", ["Q4 plan", "Offsite venue"], ["Person 1"]],
    ["subject:venue OR subject:ticket", ["Offsite venue", "Ticket #42"], []],
    ["label:receipts", ["Your receipt"], []],
    ["is:list", ["Digest: week 44", "Digest: daily", "Digest: week 43"], ["Big sale"]],
    // has: is not supported locally, so it is ignored.
    ["has:attachment venue", ["Offsite venue"], []],
    // Only in trashOld, which has no message outside trash and spam.
    ["wombat", [], ["Old trash"]],
  ];
  await inOrder(table, async ([query, include, exclude]) => {
    await search(app, query);
    await Promise.all([
      ...include.map((subject) =>
        expect(app.row(subject), `${query} finds ${subject}`).toHaveCount(1),
      ),
      ...exclude.map((subject) =>
        expect(app.row(subject), `${query} skips ${subject}`).toHaveCount(0),
      ),
    ]);
  });
  await expect(app.rows).toHaveCount(0);
  expect(await gmailSearches(gmail, since), "no Gmail query before a pause").toEqual([]);

  await search(app, "is:list digest");
  await app.advance(300);
  expect(await gmailSearches(gmail, since)).toEqual(["digest"]);
});

test("SRC-03 Gmail hits are merged in as their metadata arrives", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  const invoice = s.thread("invoiceOld").threadId;
  const addr = s.thread("addr").threadId;

  await search(app, "invoice 2019");
  await expect(app.rows).toHaveCount(0);
  await app.advance(300);
  await app.settle();
  const hit = app.row("Invoice 2019");
  await expect(hit).toHaveCount(1);
  await expect(hit).toContainText("Old Billing");
  await expect(hit).toContainText("Your invoice from 2019");
  await expect(hit.locator(".tabular")).toHaveText(/\d/);

  // Opening the hit fetches its body at once.
  await app.press("Enter");
  await expect(field(app)).not.toBeFocused();
  await app.press("Enter");
  await expect(app.conversationTitle).toHaveText("Invoice 2019");
  await expect(app.messageBody(0).locator("body")).toContainText("Your invoice from 2019.");
  const full = (await gmail.requests("threads.get", { account: A })).filter(
    (r) => r.path.endsWith(`/threads/${invoice}`) && r.params.format === "full",
  );
  expect(full).toHaveLength(1);
  expect(full[0]?.at).toBe(dates.anchor + 300);

  // A cached hit needs no fetch.
  const addrFetches = async () =>
    (await gmail.requests("threads.get", { account: A })).filter((r) =>
      r.path.endsWith(`/threads/${addr}`),
    ).length;
  const before = await addrFetches();
  const since = await mark(gmail);
  await search(app, "venue");
  await app.advance(300);
  await app.settle();
  expect(await gmailSearches(gmail, since)).toEqual(["venue"]);
  await expect(app.row("Offsite venue")).toHaveCount(1);
  expect(await addrFetches()).toBe(before);
});

test("SRC-04 the Gmail query waits for a 300 ms pause", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  // Matches "invo" but not "invoice"; only in Gmail.
  await gmail.deliver({
    account: A,
    from: "Ivo Ivanov <ivo@inv.test>",
    subject: "Invocation notes",
    text: "Notes on the invocation.",
    labels: ["CATEGORY_PERSONAL"],
    date: dates.daysAgo(200),
    poll: false,
  });

  const since = await mark(gmail);
  await app.press("/");
  await app.type("inv");
  await app.advance(299);
  expect(await gmailSearches(gmail, since)).toEqual([]);

  // Hold the metadata fetches, so invo's hits are still fetching below.
  const hold = await gmail.hold("threads.get");
  await app.type("o");
  await app.advance(300);
  expect(await gmailSearches(gmail, since)).toEqual(["invo"]);

  await app.type("ice");
  await app.advance(300);
  expect(await gmailSearches(gmail, since)).toEqual(["invo", "invoice"]);
  await hold.release();
  await app.settle();
  await expect(app.row("Invoice 2019")).toHaveCount(1);
  await expect(app.row("Invocation notes")).toHaveCount(0);
});

test("SRC-05 offline search shows cached mail and says the results are from this Mac", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  await gmail.offline(true);
  // The next poll fails, so the app knows it is offline.
  await app.advance(45_000);
  await app.press("/");
  await app.type("venue");
  await expect(app.row("Offsite venue")).toHaveCount(1);
  await expect(app.page.getByText("Results from this Mac")).toBeVisible();
});

test("SRC-06 actions on search results follow the removal table", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  await search(app, "Person");
  await expect(app.rows).toHaveCount(6);
  await app.press("Enter");
  await expect(app.highlighted).toContainText("Person 1");

  // e: stays, since the thread still matches.
  await app.press("e");
  await app.settle();
  await expect(app.row("Person 1")).toHaveCount(1);
  const p1 = (await gmail.gmail(A)).messages.find((m) => m.id === s.message("p1", 1).id);
  expect(p1?.labels).not.toContain("INBOX");

  // #: leaves, since local search skips trashed-only threads.
  await highlightRow(app, "Person 2");
  await app.press("#");
  await expect(app.row("Person 2")).toHaveCount(0);
  await app.settle();
  await expect(app.row("Person 2")).toHaveCount(0);

  // s: stays, starred.
  await highlightRow(app, "Person 3");
  await app.press("s");
  await app.settle();
  await expect(app.row("Person 3")).toHaveCount(1);
  await expect(app.row("Person 3")).toHaveAttribute("data-starred", "true");

  // y: nothing happens.
  const writes = (await gmail.requests()).length;
  const toasts = await app.toasts.count();
  await app.press("y");
  await app.settle();
  await expect(app.rows).toHaveCount(5);
  await expect(app.highlighted).toContainText("Person 3");
  await expect(app.toasts).toHaveCount(toasts);
  expect((await gmail.requests()).slice(writes)).toEqual([]);
});

test("SRC-07 Gmail results come 15 at a time; the next page loads near the oldest Gmail hit", async ({
  app,
  gmail,
}) => {
  test.slow();
  const s = await seeds.bulk(gmail, app);
  const pages = async (match: (p: Record<string, string>) => boolean) =>
    (await gmail.requests("threads.list", { account: A })).filter((r) => match(r.params));

  // The Archive2019 label view.
  await app.press("Meta+k");
  await app.palette.getByRole("option", { name: /^Go to label/ }).click();
  const picker = app.page.getByRole("dialog", { name: "Go to label" });
  await picker.getByRole("option", { name: "Archive2019" }).click();
  await app.settle();
  const label = s.label(A, "Archive2019");
  const labelPages = await pages((p) => p.labelIds === label);
  expect(labelPages[0]?.params.maxResults).toBe("15");
  await expect.poll(() => allSubjects(app), { timeout: 15_000 }).toEqual(forty("Archived"));

  // Search: every hit exists only in Gmail.
  await search(app, "invoice");
  await app.advance(300);
  await app.settle();
  const searchPages = await pages((p) => p.q === "invoice");
  expect(searchPages[0]?.params.maxResults).toBe("15");
  // The first page fits on screen: the next one is asked for without scrolling to the end.
  await expect.poll(async () => (await pages((p) => p.q === "invoice")).length).toBeGreaterThan(1);
  expect((await pages((p) => p.q === "invoice"))[1]?.params.pageToken).toBe("15");
  await expect.poll(() => allSubjects(app), { timeout: 15_000 }).toEqual(forty("Invoice"));
  // Each page is asked for once.
  const tokens = (await pages((p) => p.q === "invoice")).map((r) => r.params.pageToken ?? "");
  expect(tokens).toEqual(["", "15", "30"]);
});
