// Search (SPEC §Search): local results at once, the Gmail
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

  // Nothing but `is:list` would reach Gmail empty and list every thread, such
  // as Invoice 2019, which only Gmail has and which is not list mail.
  await inOrder(["is:list", "-is:list", "(is:list)"], async (query) => {
    await search(app, query);
    await app.advance(300);
    await app.settle();
    await expect(app.row(query === "-is:list" ? "Big sale" : "Digest: daily")).toHaveCount(1);
    await expect(app.row("Invoice 2019"), `${query} asks Gmail nothing`).toHaveCount(0);
  });
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
  await expect(app.page.getByText("Results from this Mac")).toHaveCount(0);

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

  // e: stays, since the thread still matches. Never removed and re-added.
  const p1Key = await app.rowKey("Person 1");
  const rows = await app.watchRows();
  await app.press("e");
  await app.settle();
  await expect(app.row("Person 1")).toHaveCount(1);
  await expect(app.highlighted).toContainText("Person 1");
  expect(await rows.added()).not.toContain(p1Key);
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

test("SRC-07 Gmail results come 15 at a time, and each page is asked for once", async ({
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

test("SRC-13 the next Gmail page loads near the oldest Gmail hit, not the list's end", async ({
  app,
  gmail,
}) => {
  const s = await seeds.paging(gmail, app);
  const pages = async (match: (p: Record<string, string>) => boolean) =>
    (await gmail.requests("threads.list", { account: A }))
      .filter((r) => match(r.params))
      .map((r) => r.params.pageToken ?? "");
  // Gmail's two pages of newer hits, then the local rows below them.
  const top = [
    ...Array.from({ length: 30 }, (_, i) => `Remote ${String(i + 1).padStart(2, "0")}`),
    "Local 01",
  ];
  const shown = async () => (await app.subjects()).slice(0, top.length);

  // Search: 80 older local matches sit below Gmail's first page, so its
  // oldest hit is on screen while the list's end is far below. The second
  // page loads without scrolling, and its hits land above the local rows.
  await search(app, "ledger");
  await app.advance(300);
  await app.settle();
  await expect.poll(() => pages((p) => p.q === "ledger")).toContain("15");
  await expect.poll(shown).toEqual(top);

  // The Ledgers label view pages the same way.
  await app.press("Escape");
  await app.press("Meta+k");
  await app.palette.getByRole("option", { name: /^Go to label/ }).click();
  const picker = app.page.getByRole("dialog", { name: "Go to label" });
  await picker.getByRole("option", { name: "Ledgers" }).click();
  await app.settle();
  const label = s.label(A, "Ledgers");
  await expect.poll(() => pages((p) => p.labelIds === label)).toContain("15");
  await expect.poll(shown).toEqual(top);
});

test("SRC-08 a sent reply is found by local search", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  await app.row("Offsite venue").click();
  await expect(app.conversationTitle).toHaveText("Offsite venue");
  await app.settle();
  await app.reply("r");
  await app.type("Zanzibar works for me");
  await app.settle();
  await app.press("Meta+Enter");
  await expect(app.editor).toHaveCount(0);
  await app.advance(10_000);
  await app.settle();
  await app.press("Escape");

  // Offline, so only the local index can find it.
  await gmail.offline(true);
  await app.advance(45_000);
  await search(app, "zanzibar");
  await expect(app.row("Offsite venue")).toHaveCount(1);
});

test("SRC-09 search takes the header as in Superhuman, marks matches, and lists tips", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  await app.press("/");
  await expect(field(app)).toBeFocused();
  // In line with the senders below, larger than a row, and hints that work while typing.
  const left = (sel: string) =>
    app.page
      .locator(sel)
      .first()
      .evaluate((el) => Math.round(el.getBoundingClientRect().left));
  expect(
    Math.abs((await left("input[placeholder=Search]")) - (await left("[role=option] .w-sender"))),
  ).toBeLessThanOrEqual(1);
  await expect(field(app)).toHaveCSS("font-size", "16px");
  await expect(app.page.getByText("Enter to see results")).toBeVisible();
  await expect(app.page.getByRole("complementary", { name: "Search tips" })).toContainText("from:");

  // Words are marked in the results; an operator's value is not.
  await field(app).fill("from:dave plan");
  await app.settle();
  await expect(app.row("Q4 plan")).toHaveCount(1);
  const marks = await app.row("Q4 plan").locator("mark").allTextContents();
  expect(marks.length).toBeGreaterThan(0);
  expect(marks.every((m) => m.toLowerCase() === "plan")).toBe(true);

  // The close button ends the search.
  await app.page.getByRole("button", { name: "Close search (Esc)" }).click();
  await expect(field(app)).toHaveCount(0);
  await expect(app.page.getByRole("complementary", { name: "Search tips" })).toHaveCount(0);
  await expect(app.splitTitles().first()).toBeVisible();
});

// A listing run on the account worker's tasks died with it, leaving "Searching Gmail…" up forever.
test("SRC-10 a Gmail search stops searching when its account signs in again under it", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  // The search's listing is on its way to Gmail when A's worker is replaced.
  const listing = await gmail.hold({ api: "threads.list", query: { q: "zebra crossing" } });
  await search(app, "zebra crossing");
  await app.advance(300);
  await expect
    .poll(async () => (await gmail.requests("threads.list")).some((r) => r.held))
    .toBe(true);
  await expect(app.page.getByText("Searching Gmail…")).toBeVisible();

  const lists = (await gmail.requests("threads.list", { account: A })).length;
  await gmail.nextSignIn({ email: A });
  // Enter leaves the field for the results, where the palette opens.
  await app.press("Enter");
  await expect(field(app)).not.toBeFocused();
  await app.press("Meta+k");
  await expect(app.palette).toBeVisible();
  await app.type("Add account");
  await expect(app.palette.locator("[cmdk-item]").first()).toContainText("Add account");
  await app.press("Enter");
  // The new worker is running once it lists Inbox again.
  await expect
    .poll(async () => (await gmail.requests("threads.list", { account: A })).length)
    .toBeGreaterThan(lists);
  // The old worker's listing went with it: nothing is parked any more.
  await listing.release(undefined, { min: 0 });
  await app.settle();

  await expect(app.page.getByText("Searching Gmail…")).toHaveCount(0);
  await expect(app.page.getByText("Nothing here.")).toBeVisible();
});

// Local search matched everything on an operator it didn't know, so `in:sent venue` listed incoming mail.
test("SRC-11 local search applies Gmail's operators before Gmail answers", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  // Cache trashOld through the Trash view, so in:trash has something to find.
  await app.press("Meta+k");
  await app.palette.getByRole("option", { name: /^Go to Trash/ }).click();
  await expect(app.row("Old trash")).toHaveCount(1);
  await app.page
    .getByRole("navigation")
    .getByRole("button", { name: "Inbox", exact: true })
    .click();
  // Gmail's own answer is right; parked, it can't hide what local search shows.
  await gmail.hold("threads.list");

  // Mail dates: the clock is Monday 2026-11-02 10:00 in New York. Contract
  // came at 00:00 that day, Links at 23:00 the day before.
  const table: [string, string[], string[]][] = [
    // The reported bug: in:sent listed incoming mail with the word.
    ["in:sent venue", [], ["Offsite venue"]],
    [
      "in:sent",
      ["Q4 plan", "Budget draft", "Alias note", "Sent 10 days ago", "Sent 89 days ago"],
      ["Offsite venue", "Person 1", "Ticket #42"],
    ],
    ["is:unread", ["Offsite venue", "Person 1", "Q4 plan"], ["Ticket #42", "Person 2", "Big sale"]],
    ["is:read person", ["Person 2", "Person 4"], ["Person 1", "Person 3"]],
    ["has:attachment", ["Contract"], ["Offsite venue", "Person 1"]],
    ["is:starred", ["Trip ideas"], ["Person 1"]],
    ["-in:inbox", ["Trip ideas", "Sent 10 days ago"], ["Person 1", "Offsite venue"]],
    ["after:2026/11/02", ["Offsite venue", "Contract"], ["Links", "Trip ideas"]],
    ["before:2026-11-02", ["Links", "Trip ideas"], ["Offsite venue", "Contract"]],
    ["newer_than:2d", ["Offsite venue", "Latest rule"], ["Trip ideas", "Sent 10 days ago"]],
    ["older_than:1m", ["Sent 89 days ago"], ["Trip ideas", "Offsite venue"]],
    ["from:carol -venue", ["Q4 plan"], ["Offsite venue"]],
    ["-(is:unread OR is:starred) person", ["Person 2", "Person 4"], ["Person 1", "Person 3"]],
    ["in:trash wombat", ["Old trash"], []],
  ];
  await inOrder(table, async ([query, include, exclude]) => {
    await search(app, query);
    await app.advance(300);
    await app.settle();
    await Promise.all([
      ...include.map((subject) =>
        expect(app.row(subject), `${query} finds ${subject}`).toHaveCount(1),
      ),
      ...exclude.map((subject) =>
        expect(app.row(subject), `${query} skips ${subject}`).toHaveCount(0),
      ),
    ]);
  });
});

test("SRC-12 a query local search can't evaluate waits for Gmail, and offline says it needs Gmail", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  const listing = await gmail.hold("threads.list");
  // `filename:` needs attachment names, which only Gmail has: local search
  // must not list every venue mail as if the operator matched, or as if it
  // never did.
  await search(app, "venue -filename:pdf");
  await app.advance(300);
  await app.settle();
  await expect(app.row("Offsite venue")).toHaveCount(0);
  await expect(app.rows).toHaveCount(0);
  await expect(app.page.getByText("Searching Gmail…")).toBeVisible();
  // Gmail's hits are listed as usual once it answers.
  await listing.release();
  await app.settle();
  await expect(app.row("Offsite venue")).toHaveCount(1);
  await expect(app.page.getByText("Searching Gmail…")).toHaveCount(0);
  await app.press("Escape");

  await gmail.offline(true);
  // The next poll fails, so the app knows it is offline.
  await app.advance(45_000);
  await search(app, "filename:pdf venue");
  await expect(app.page.getByText("This search needs Gmail")).toBeVisible();
  await expect(app.page.getByText("Nothing here.")).toHaveCount(0);
  await expect(app.rows).toHaveCount(0);
  // A query local search can evaluate still lists cached mail.
  await search(app, "has:attachment");
  await expect(app.row("Contract")).toHaveCount(1);
  await expect(app.page.getByText("This search needs Gmail")).toHaveCount(0);
});

test("SRC-14 a search or Gmail-listed view opened offline lists from Gmail when the network returns", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  const projects = s.label(A, "Projects");
  /** A's Gmail listings since the mark. */
  const listings = async (since: number) =>
    (await gmail.requests())
      .slice(since)
      .filter((r) => r.api === "threads.list" && r.account === A);
  await gmail.offline(true);
  // The next poll fails, so the app knows it is offline.
  await app.advance(45_000);
  const offline = await mark(gmail);

  // A replaced search and a closed one never reach Gmail; the view still
  // open does, once Gmail can be reached.
  await search(app, "zebra crossing");
  await app.advance(300);
  await search(app, "invoice 2019");
  await app.advance(300);
  await app.press("Escape");
  await app.press("g", "l");
  await app.type("Projects");
  await app.press("Enter");
  await app.settle();
  await expect(app.row("Project alpha")).toHaveCount(0);
  await gmail.offline(false);
  await app.settle();
  await expect(app.row("Project alpha")).toHaveCount(1);
  await expect(app.row("Project beta")).toHaveCount(1);
  const back = await listings(offline);
  expect(back.filter((r) => r.params.labelIds === projects)).toHaveLength(1);
  expect(back.flatMap((r) => r.params.q ?? []).filter((q) => /zebra|invoice/.test(q))).toEqual([]);

  // A search open while offline searches Gmail again once it can.
  await gmail.offline(true);
  await app.advance(45_000);
  const offlineAgain = await mark(gmail);
  await search(app, "invoice 2019");
  await app.advance(300);
  await app.settle();
  await expect(app.rows).toHaveCount(0);
  await expect(app.page.getByText("Results from this Mac")).toBeVisible();
  await expect(app.page.getByText("Searching Gmail…")).toHaveCount(0);
  const listing = await gmail.hold({ api: "threads.list", query: { q: "invoice 2019" } });
  await gmail.offline(false);
  await expect
    .poll(async () => (await gmail.requests("threads.list")).some((r) => r.held))
    .toBe(true);
  await expect(app.page.getByText("Searching Gmail…")).toBeVisible();
  await expect(app.page.getByText("Results from this Mac")).toHaveCount(0);
  await listing.release();
  await app.settle();
  await expect(app.row("Invoice 2019")).toHaveCount(1);
  await expect(app.page.getByText("Searching Gmail…")).toHaveCount(0);
  expect((await listings(offlineAgain)).filter((r) => r.params.q === "invoice 2019")).toHaveLength(
    1,
  );
});

test("SRC-15 an open search runs once Gmail can be reached again, though nothing else calls Gmail", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  // The network goes away between polls, and the search is the first to find out.
  await gmail.offline(true);
  await search(app, "invoice 2019");
  await app.advance(300);
  await app.settle();
  await expect(app.page.getByText("Results from this Mac")).toBeVisible();
  await expect(app.rows).toHaveCount(0);

  // The network returns, and nothing tells the app: no action or send is
  // waiting, and the next poll is 45 s away.
  await gmail.offline(false, { probe: true });
  await app.settle();
  await expect(app.page.getByText("Results from this Mac")).toBeVisible();
  await app.advance(10_000);
  await app.settle();
  await expect(app.row("Invoice 2019")).toHaveCount(1);
  await expect(app.page.getByText("Results from this Mac")).toHaveCount(0);
});

test("SRC-16 a conversation opened from a search in Trash or Spam lists their messages", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  // Synced now, so local search finds it; Old trash comes from Gmail's hits.
  await gmail.deliver({
    account: A,
    from: "Tess Toss <tess@toss.test>",
    subject: "Tossed today",
    text: "A koala was here.",
    labels: ["TRASH"],
  });
  await app.settle();
  await search(app, "in:trash");
  await app.advance(300);
  await expect(app.row("Old trash")).toHaveCount(1);
  expect(await app.subjects()).toEqual(["Tossed today", "Latest rule", "Old trash"]);

  await app.press("Enter", "Enter");
  await expect(app.conversationTitle).toHaveText("Tossed today");
  await expect(app.messageBody(0).locator("body")).toContainText("A koala was here.");
  // `j` opens the next results the same way: Latest rule with its trashed reply.
  await app.press("j");
  await expect(app.conversationTitle).toHaveText("Latest rule");
  await expect(app.messages).toHaveCount(2);
  await app.press("j");
  await expect(app.conversationTitle).toHaveText("Old trash");
  await expect(app.messageBody(0).locator("body")).toContainText("A wombat was here.");
  // The reply keys work there.
  await app.reply("r");
  await app.press("Escape", "Escape");
  await expect(app.editor).toHaveCount(0);

  // A search that does not look in Trash hides the trashed reply, as Inbox does (CONV-11).
  await search(app, "latest rule");
  await app.row("Latest rule").click();
  await expect(app.conversationTitle).toHaveText("Latest rule");
  await app.settle();
  await expect(app.messages).toHaveCount(1);
});
