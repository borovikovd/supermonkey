// The latest-message rule, view membership by message labels, Done, Drafts,
// and Label / Trash / Spam views that also list from Gmail (COVERAGE 5.3).
import type { App } from "./app.ts";
import type { Delivery } from "./control.ts";
import { expect, test } from "./fixtures.ts";
import * as seeds from "./seeds.ts";
import { A, dates } from "./seeds.ts";

/** Opens a view with its `g` jump and waits for its title. */
async function go(app: App, key: string, title: string | null): Promise<void> {
  await app.press("g", key);
  if (title === null) await expect(app.splitTitles().first()).toBeVisible();
  else await expect(app.page.locator("div:has(> button[title='Search (/)'])")).toHaveText(title);
  await app.settle();
}

/** Runs `step` on each item, one after another. */
async function inOrder<T>(items: T[], step: (item: T) => Promise<void>): Promise<void> {
  await items.reduce<Promise<void>>((prev, item) => prev.then(() => step(item)), Promise.resolve());
}

/** Two messages on A from Vic: `first`, then a reply carrying `second`. `at` is the batch index of the first. */
function pair(subject: string, first: string[], second: string[], at: number): Delivery[] {
  const msg = (n: number, labels: string[]): Delivery => ({
    account: A,
    from: "Vic View <vic@view.test>",
    subject: n === 1 ? subject : `Re: ${subject}`,
    text: `${subject} ${n}.`,
    labels,
    date: dates.minutesAgo(4 - n),
    poll: false,
  });
  return [msg(1, first), { ...msg(2, second), after: at }];
}

/** Highlights the row containing `text` with j / k from the current highlight. */
async function highlight(app: App, text: string): Promise<void> {
  const keys = await app.rows.evaluateAll((rows, t) => {
    const at = rows.findIndex((r) => r.getAttribute("aria-selected") === "true");
    const to = rows.findIndex((r) => (r.textContent ?? "").includes(t));
    return { at, to };
  }, text);
  if (keys.to < 0) throw new Error(`no row with ${text}`);
  const delta = keys.to - keys.at;
  await app.press(...Array.from({ length: Math.abs(delta) }, () => (delta > 0 ? "j" : "k")));
  await expect(app.highlighted).toContainText(text);
}

test("VIEW-01 a thread's latest message sets its row", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  // A thread dated between trashedLatest's m1 (24 h ago) and its trashed m2 (20 h ago).
  await gmail.deliver({
    account: A,
    from: "Sam Sort <sam@sort.test>",
    subject: "Sort probe",
    text: "Between the two.",
    labels: ["INBOX", "CATEGORY_PERSONAL"],
    date: dates.hoursAgo(22),
    poll: false,
  });
  // m1 from Oli in Inbox, m2 a newer draft by A.
  const oli = await gmail.deliver({
    account: A,
    from: "Oli Oak <oli@oak.test>",
    subject: "Oak tree",
    text: "Oli writes first.",
    labels: ["INBOX", "CATEGORY_PERSONAL"],
    date: dates.minutesAgo(5),
    poll: false,
  });
  await gmail.createDraft(A, {
    to: ["Oli Oak <oli@oak.test>"],
    subject: "Re: Oak tree",
    threadId: oli.threadId,
    inReplyTo: oli.messageId,
    html: "<p>Ana drafts a reply.</p>",
    date: dates.minutesAgo(1),
  });
  // m1 from Pat and m2 from Rex, both spam.
  await gmail.deliverMany([
    {
      account: A,
      from: "Pat Spam <pat@junk.test>",
      subject: "Cheap pills",
      text: "Pat spam text.",
      labels: ["SPAM"],
      date: dates.hoursAgo(3),
      poll: false,
    },
    {
      account: A,
      from: "Rex Spam <rex@junk.test>",
      subject: "Re: Cheap pills",
      text: "Rex spam text.",
      labels: ["SPAM"],
      date: dates.hoursAgo(2),
      after: 0,
      poll: false,
    },
  ]);
  // A thread that is only a draft.
  await gmail.createDraft(A, {
    to: ["Kim Kato <kim@kite.test>"],
    subject: "Idea",
    html: "<p>An idea for later.</p>",
  });
  await gmail.poll();
  await app.settle();

  // trashedLatest: m1 (Mia) sets the row, and it sorts by m1's time.
  const latest = app.row("Latest rule");
  await expect(latest).toContainText("Mia Moss");
  await expect(latest).toContainText("The first word.");
  await expect(latest).not.toContainText("Ned");
  const important = (await app.sections())[0]?.subjects ?? [];
  expect(important.indexOf("Sort probe")).toBeGreaterThanOrEqual(0);
  expect(important.indexOf("Latest rule")).toBe(important.indexOf("Sort probe") + 1);

  // A newer draft does not set the row.
  const oak = app.row("Oak tree");
  await expect(oak).toContainText("Oli Oak");
  await expect(oak).toContainText("Oli writes first.");
  await expect(oak).not.toContainText("Ana drafts a reply.");

  // In Spam, no message qualifies: the newest non-draft message (Rex) is used.
  await go(app, "!", "Spam");
  const pills = app.row("Cheap pills");
  await expect(pills).toHaveCount(1);
  await expect(pills).toContainText("Rex Spam");
  await expect(pills).toContainText("Rex spam text.");

  // A thread with only a draft lists the draft in Drafts.
  await go(app, "d", "Drafts");
  await expect(app.row("Idea")).toContainText("An idea for later.");
});

test("VIEW-02 view membership comes from message labels", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  const deliveries = [
    ...pair("Trashed reply", ["INBOX", "CATEGORY_PERSONAL"], ["TRASH"], 0),
    ...pair("Trashed star", ["INBOX", "CATEGORY_PERSONAL"], ["STARRED", "TRASH"], 2),
    ...pair("Spam inbox", ["STARRED", "CATEGORY_PERSONAL"], ["INBOX", "SPAM"], 4),
  ];
  await gmail.deliverMany(deliveries);
  await gmail.poll();
  await app.settle();

  const has = async (subject: string, n: number) => expect(app.row(subject)).toHaveCount(n);

  // Inbox: INBOX on a message outside trash and spam.
  await has("Trashed reply", 1);
  await has("Trashed star", 1);
  await has("Spam inbox", 0);

  await go(app, "#", "Trash");
  await has("Trashed reply", 1);
  await has("Trashed star", 1);
  await has("Spam inbox", 0);

  await go(app, "!", "Spam");
  await has("Spam inbox", 1);
  await has("Trashed reply", 0);
  await has("Trashed star", 0);

  // Starred: the star on a trashed message does not count.
  await go(app, "s", "Starred");
  await has("Spam inbox", 1);
  await has("Trashed star", 0);
  await has("Trashed reply", 0);
});

test("VIEW-03 Done lists every thread outside Inbox, from Gmail too", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  await expect(app.highlighted).toContainText("Person 1");

  // e on p1 in this app.
  await app.press("e");
  await expect(app.highlighted).toContainText("Person 2");
  // p3 snoozed with h.
  await app.press("j");
  await expect(app.highlighted).toContainText("Person 3");
  await app.press("h");
  await expect(app.page.getByRole("dialog", { name: /Snooze until/ })).toBeVisible();
  await app.type("fri 9am");
  await expect(app.palette.locator("[cmdk-item]").first()).toBeVisible();
  await app.press("Enter");
  await expect(app.row("Person 3")).toHaveCount(0);
  // p4 and p5 archived.
  await expect(app.highlighted).toContainText("Person 4");
  await app.press("e");
  await expect(app.highlighted).toContainText("Person 5");
  await app.press("e");
  // A thread that arrives already outside Inbox.
  await gmail.deliver({
    account: A,
    from: "Nell Never <nell@never.test>",
    subject: "Never in Inbox",
    labels: ["CATEGORY_PERSONAL"],
    poll: false,
  });
  // p2 archived by another client.
  await gmail.modify(A, s.latest("p2").id, [], ["INBOX"], { poll: true });
  await app.settle();
  await expect(app.row("Person 2")).toHaveCount(0);

  await go(app, "e", "Done");
  await expect(app.row("Person 1")).toHaveCount(1);
  await expect(app.row("Person 2")).toHaveCount(1);
  await expect(app.row("Person 4")).toHaveCount(1);
  await expect(app.row("Person 5")).toHaveCount(1);
  await expect(app.row("Never in Inbox")).toHaveCount(1);
  // Archived before the first connect: listed from Gmail's archive.
  await expect(app.row("Archived before")).toHaveCount(1);
  const done = (await gmail.requests("threads.list", { account: A })).filter(
    (r) => r.params.q === "-in:inbox -in:draft",
  );
  expect(done.length).toBeGreaterThanOrEqual(1);
  expect(done[0]?.params.maxResults).toBe("15");
  // Snoozed is not Done; nor are Trash and Spam.
  await expect(app.row("Person 3")).toHaveCount(0);
  await expect(app.row(/Old trash|Old spam/)).toHaveCount(0);

  // # from Done: every message is in trash, so it leaves Done for Trash.
  await highlight(app, "Person 4");
  await app.press("#");
  await expect(app.row("Person 4")).toHaveCount(0);
  // Shift+E from Done: back in Inbox.
  await highlight(app, "Person 5");
  await app.press("Shift+E");
  await expect(app.row("Person 5")).toHaveCount(0);
  await app.settle();
  await expect(app.row("Person 1")).toHaveCount(1);
  await expect(app.row("Person 2")).toHaveCount(1);
  await expect(app.row("Person 4")).toHaveCount(0);
  await expect(app.row("Person 5")).toHaveCount(0);

  await go(app, "h", "Snoozed");
  await expect(app.row("Person 3")).toHaveCount(1);
  await go(app, "#", "Trash");
  await expect(app.row("Person 4")).toHaveCount(1);
  await go(app, "i", null);
  await expect(app.row("Person 5")).toHaveCount(1);
  await expect(app.row("Person 3")).toHaveCount(0);
});

test("VIEW-04 Drafts lists every draft, and mail actions do nothing there", async ({
  app,
  gmail,
}) => {
  await seeds.drafts(gmail, app);
  await go(app, "d", "Drafts");

  // One row per draft: dTwoA and dTwoB are two rows on one thread.
  await expect(app.rows).toHaveCount(7);
  await expect(app.row("Re: Person 2")).toHaveCount(2);
  await expect(app.row("dTwoA")).toHaveCount(1);
  await expect(app.row("dTwoB")).toHaveCount(1);
  // The hint bar offers what works here, not the mail actions that do nothing.
  await expect(app.page.getByText("Enter to open draft")).toBeVisible();
  await expect(app.page.getByText("E to Mark Done")).toHaveCount(0);

  // A draft composed offline is listed before it reaches Gmail.
  await gmail.offline(true);
  await app.compose();
  await app.subject.fill("Offline idea");
  await app.settle();
  await app.press("Escape", "Escape");
  await expect(app.row("Offline idea")).toHaveCount(1);
  await expect(app.rows).toHaveCount(8);
  expect((await gmail.gmail(A)).drafts).toHaveLength(7);
  await gmail.offline(false);
  await app.settle();
  await expect.poll(async () => (await gmail.gmail(A)).drafts.length).toBe(8);

  // Sending a draft removes its row at once; it stays gone while the send waits.
  await highlight(app, "An idea for later");
  await app.press("Enter");
  await expect(app.page.getByRole("heading", { name: "New Message" })).toBeVisible();
  await app.settle();
  await app.press("Meta+Enter");
  await expect(app.row("An idea for later")).toHaveCount(0);
  await app.settle();
  await expect(app.row("An idea for later")).toHaveCount(0);
  await expect(app.rows).toHaveCount(7);
  // Undoing the send brings the row back.
  await app.toast("Sending").getByRole("button", { name: "Undo" }).click();
  await expect(app.page.getByRole("heading", { name: "New Message" })).toBeVisible();
  await app.settle();
  await app.press("Escape", "Escape");
  await expect(app.row("An idea for later")).toHaveCount(1);
  await expect(app.rows).toHaveCount(8);

  // Discarding a draft removes its row.
  await highlight(app, "Aimed at nobody");
  await app.press("Enter");
  await expect(app.editor).toBeVisible();
  await app.settle();
  await app.page.getByTitle("Discard (⌘⇧,)").click();
  await app.settle();
  await app.press("Escape");
  await expect(app.list).toBeVisible();
  await expect(app.row("Aimed at nobody")).toHaveCount(0);
  await expect(app.rows).toHaveCount(7);

  // Mail actions do nothing in Drafts.
  const subjects = await app.subjects();
  const modifies = (await gmail.requests("messages.batchModify")).length;
  const trashes = (await gmail.requests("messages.trash")).length;
  await app.press("j");
  const target = (await app.highlighted.textContent()) ?? "";
  const keys = ["e", "#", "!", "s", "u", "y", "Shift+E"];
  await inOrder(keys, async (key) => {
    await app.press(key);
    await app.settle();
    expect(await app.subjects(), key).toEqual(subjects);
  });
  // h and l: even a choice in their pickers changes nothing.
  await inOrder(["h", "l"], async (key) => {
    await app.press(key);
    await app.settle();
    if (await app.palette.isVisible()) {
      await expect(app.palette.locator("[cmdk-item]").first()).toBeVisible();
      await app.press("Enter");
    }
    await app.press("Escape");
    await app.settle();
    expect(await app.subjects(), key).toEqual(subjects);
  });
  await expect(app.highlighted).toHaveText(target);
  await expect(app.rows).toHaveCount(7);
  expect(await gmail.requests("messages.batchModify")).toHaveLength(modifies);
  expect(await gmail.requests("messages.trash")).toHaveLength(trashes);
});

test("VIEW-05 Label, Trash, and Spam views also list from Gmail", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  const projects = s.label(A, "Projects");
  // The connect sync did not fetch these.
  await expect(app.row("Project alpha")).toHaveCount(0);

  await app.press("g", "l");
  await app.type("Projects");
  await app.press("Enter");
  await app.settle();
  await expect(app.row("Project alpha")).toHaveCount(1);
  await expect(app.row("Project beta")).toHaveCount(1);
  await expect(app.row("Project gamma")).toHaveCount(1);
  const labelList = (await gmail.requests("threads.list", { account: A })).filter(
    (r) => r.params.labelIds === projects,
  );
  expect(labelList.length).toBeGreaterThanOrEqual(1);
  expect(labelList[0]?.params.maxResults).toBe("15");

  await go(app, "#", "Trash");
  await expect(app.row("Old trash")).toHaveCount(1);
  await go(app, "!", "Spam");
  await expect(app.row("Old spam")).toHaveCount(1);
  const lists = await gmail.requests("threads.list", { account: A });
  const trashList = lists.find((r) => r.params.labelIds === "TRASH");
  const spamList = lists.find((r) => r.params.labelIds === "SPAM");
  expect(trashList?.params.includeSpamTrash).toBe("true");
  expect(spamList?.params.includeSpamTrash).toBe("true");

  // A fetched thread opens with its body, fetched on open.
  await app.row("Old spam").click();
  await expect(app.conversationTitle).toHaveText("Old spam");
  await expect(app.messageBody(0).getByText("You won a prize.")).toBeVisible();

  // Offline, Projects shows the cached rows and no error.
  await app.press("Escape");
  await gmail.offline(true);
  await app.press("g", "l");
  await app.type("Projects");
  await app.press("Enter");
  await app.settle();
  await expect(app.row("Project alpha")).toHaveCount(1);
  await expect(app.row("Project beta")).toHaveCount(1);
  await expect(app.row("Project gamma")).toHaveCount(1);
  await expect(app.errorToasts).toHaveCount(0);
});

test("VIEW-06 All Mail lists every thread outside Trash and Spam, from Gmail too", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  await go(app, "a", "All Mail");
  // Inbox mail, and the archive from before the first connect, listed from Gmail.
  await expect(app.row("Person 1")).toHaveCount(1);
  await expect(app.row("Archived before")).toHaveCount(1);
  await expect(app.row(/Old trash|Old spam/)).toHaveCount(0);
  const all = (await gmail.requests("threads.list", { account: A })).filter(
    (r) => r.params.q === "-in:draft",
  );
  expect(all.length).toBeGreaterThanOrEqual(1);
  expect(all[0]?.params.maxResults).toBe("15");
  expect(all[0]?.params.includeSpamTrash).toBeUndefined();

  // Archived mail is still All Mail: e marks the row read and keeps it. # takes it out.
  await highlight(app, "Person 1");
  await app.press("e");
  await app.settle();
  await expect(app.row("Person 1")).toHaveCount(1);
  await expect(app.row("Person 1")).not.toHaveAttribute("data-unread");
  await app.press("#");
  await expect(app.row("Person 1")).toHaveCount(0);

  // The palette goes there too.
  await go(app, "e", "Done");
  await app.press("Meta+k");
  await app.type("Go to All Mail");
  await app.press("Enter");
  await expect(app.page.locator("div:has(> button[title='Search (/)'])")).toHaveText("All Mail");
});
