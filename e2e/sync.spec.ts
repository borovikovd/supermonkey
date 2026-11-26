import type { App } from "./app.ts";
import type { GmailMessage, GmailState } from "./control.ts";
import { expect, test } from "./fixtures.ts";
import * as seeds from "./seeds.ts";
import { A, A_ALIAS, B } from "./seeds.ts";

async function goToLabel(app: App, name: string): Promise<void> {
  await app.press("g", "l");
  await expect(app.palette).toBeVisible();
  await app.type(name);
  await expect(app.palette.locator("[cmdk-item]").first()).toHaveText(name);
  await app.press("Enter");
  await expect(app.palette).toHaveCount(0);
  await app.settle();
}

const live = (m: GmailMessage) => !m.labels.includes("TRASH") && !m.labels.includes("SPAM");

const bare = (subject: string) => subject.replace(/^((re|fwd?):\s*)+/i, "");

function gmailInbox(state: GmailState): { subject: string; date: number }[] {
  const threads = new Map<string, GmailMessage[]>();
  for (const m of state.messages.filter((x) => !x.labels.includes("DRAFT")))
    threads.set(m.threadId, [...(threads.get(m.threadId) ?? []), m]);
  return Array.from(threads.values())
    .filter((msgs) => msgs.some((m) => live(m) && m.labels.includes("INBOX")))
    .map((msgs) => {
      const sorted = msgs.toSorted((a, b) => a.date - b.date);
      const latest = sorted.findLast(live) ?? sorted.at(-1);
      return { subject: bare(sorted[0]?.headers.subject ?? ""), date: latest?.date ?? 0 };
    })
    .toSorted((a, b) => b.date - a.date);
}

async function expectInboxMatchesGmail(app: App, email: string): Promise<void> {
  const gmail = gmailInbox(await app.gmail.gmail(email));
  const dates = new Map(gmail.map((t) => [t.subject, t.date]));
  await expect
    .poll(async () => (await app.sections()).flatMap((s) => s.subjects.map(bare)).toSorted())
    .toEqual(gmail.map((t) => t.subject).toSorted());
  for (const section of await app.sections()) {
    const own = section.subjects.map((x) => dates.get(bare(x)) ?? 0);
    expect(own, section.name).toEqual(own.toSorted((a, b) => b - a));
  }
  const counts = (await app.splitHeader()).match(/\d+/g)?.map(Number) ?? [];
  expect(counts.reduce((a, b) => a + b, 0)).toBe(gmail.length);
}

test("SYN-01 the connect sync matches Gmail", async ({ app, gmail, page }) => {
  await page.setViewportSize({ width: 1280, height: 2000 });
  const s = await seeds.drafts(gmail, app);

  await expectInboxMatchesGmail(app, A);

  await app.press("l");
  await expect(app.palette).toBeVisible();
  await expect(app.palette.locator("[cmdk-item]")).toHaveCount(4);
  expect((await app.palette.locator("[cmdk-item]").allInnerTexts()).toSorted()).toEqual([
    "Projects",
    "Receipts",
    "Travel",
    "Work/Clients",
  ]);
  await app.press("Escape");

  await app.compose();
  await app.press("Meta+Shift+F");
  await expect(app.palette).toBeVisible();
  await expect(app.palette.locator("[cmdk-item]")).toHaveCount(2);
  await expect(app.palette.locator("[cmdk-item]").filter({ hasText: A })).toHaveCount(1);
  await expect(app.palette.locator("[cmdk-item]").filter({ hasText: A_ALIAS })).toHaveCount(1);
  await app.press("Escape");
  await app.press("Escape", "Escape");
  await expect(app.page.getByRole("heading", { name: "New Message" })).toHaveCount(0);
  await expect(app.account(B)).toContainText("Z");

  await app.press("g", "d");
  await app.settle();
  await expect(app.rows).toHaveCount(s.drafts.size);
  await expect(app.row("Idea")).toHaveCount(1);
  await expect(app.row("Fwd: Contract")).toHaveCount(1);
  await expect(app.row("Re: Person 2")).toHaveCount(2);

  await app.press("g", "t");
  await app.settle();
  await expect(app.row("Sent 10 days ago")).toHaveCount(1);
  await expect(app.row("Sent 89 days ago")).toHaveCount(1);
  await expect(app.row("Sent 91 days ago")).toHaveCount(0);

  await app.press("g", "s");
  await app.settle();
  await expect(app.row("Trip ideas")).toHaveCount(1);

  const fetchedThreads = (await gmail.requests("threads.get", { account: A })).map((r) => r.path);
  for (const key of ["archivedOld", "projOld1", "invoiceOld"]) {
    const id = s.thread(key).threadId;
    expect(
      fetchedThreads.filter((p) => p.endsWith(`/threads/${id}`)),
      key,
    ).toEqual([]);
  }

  const fetched = (await gmail.requests("attachments.get")).map((r) => r.path);
  expect(fetched).toEqual([
    expect.stringContaining(`/messages/${s.message("fwd", 1).id}/attachments/`),
  ]);
});

test("SYN-02 mail that changed while the app was closed shows after relaunch", async ({
  app,
  gmail,
  page,
}) => {
  await page.setViewportSize({ width: 1280, height: 2000 });
  const s = await seeds.standard(gmail, app);
  const p1 = s.message("p1", 1);
  await app.settle();

  await app.restart(async () => {
    await gmail.deliver({
      account: A,
      from: "Nia New <nia@new.test>",
      subject: "While closed",
      text: "Arrived while the app was closed.",
      labels: ["INBOX", "UNREAD", "CATEGORY_PERSONAL"],
      date: seeds.ANCHOR - 60_000,
      poll: false,
    });
    await gmail.deliver({
      account: A,
      from: seeds.PERSONS[0] ?? "",
      subject: "Re: Person 1",
      text: "A reply while closed.",
      threadId: p1.threadId,
      inReplyTo: p1.messageId,
      labels: ["INBOX", "UNREAD", "CATEGORY_PERSONAL"],
      date: seeds.ANCHOR,
      poll: false,
    });
    await gmail.modify(A, s.message("p2", 1).id, [], ["INBOX"]);
    await gmail.modify(A, s.message("p3", 1).id, ["STARRED"], []);
  });
  await app.settle();

  await expectInboxMatchesGmail(app, A);
  await expect(app.row(/While closed/)).toHaveCount(1);
  const top = (await app.sections())[0]?.subjects.slice(0, 2);
  expect(top?.map(bare)).toEqual(["Person 1", "While closed"]);
  await expect(app.row("Person 1")).toContainText("A reply while closed.");
  await expect(app.row("Person 2")).toHaveCount(0);
  await expect(app.row("Person 3")).toHaveAttribute("data-starred");
  await app.press("g", "e");
  await app.settle();
  await expect(app.row("Person 2")).toHaveCount(1);
});

test("SYN-03 polls bring in changes made elsewhere", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  const polls = async () => (await gmail.requests("history.list", { account: A })).length;
  const before = await polls();
  const m = (key: string) => s.message(key, 1).id;
  await gmail.modify(A, m("p1"), [], ["INBOX"]);
  await gmail.modify(A, m("p2"), ["STARRED"], []);
  await gmail.modify(A, m("p3"), [], ["UNREAD"]);
  await gmail.modify(A, m("p4"), [s.label(A, "Travel")], []);
  await gmail.deliver({
    account: A,
    from: seeds.PERSONS[4] ?? "",
    subject: "Re: Person 5",
    text: "Tom again.",
    threadId: s.thread("p5").threadId,
    inReplyTo: s.message("p5", 1).messageId,
    labels: ["INBOX", "UNREAD", "CATEGORY_PERSONAL"],
    poll: false,
  });
  await gmail.modify(A, m("p6"), ["TRASH"], ["INBOX"]);

  await app.advance(44_000);
  await app.settle();
  expect(await polls()).toBe(before);
  await expect(app.row("Person 1")).toHaveCount(1);
  await expect(app.row("Person 2")).not.toHaveAttribute("data-starred");
  await expect(app.row("Person 3")).toHaveAttribute("data-unread");
  await expect(app.row("Person 6")).toHaveCount(1);
  expect((await app.sections())[0]?.subjects[0]).toBe("Person 1");

  await app.advance(1_000);
  await app.settle();
  expect(await polls()).toBe(before + 1);
  await expect(app.row("Person 1")).toHaveCount(0);
  await expect(app.row("Person 2")).toHaveAttribute("data-starred");
  await expect(app.row("Person 3")).not.toHaveAttribute("data-unread");
  expect(bare((await app.sections())[0]?.subjects[0] ?? "")).toBe("Person 5");
  await expect(app.row("Person 5")).toHaveAttribute("data-unread");
  await expect(app.row("Person 6")).toHaveCount(0);
  await app.press("g", "e");
  await app.settle();
  await expect(app.row("Person 1")).toHaveCount(1);
  await app.press("g", "#");
  await app.settle();
  await expect(app.row("Person 6")).toHaveCount(1);
  await goToLabel(app, "Travel");
  await expect(app.row("Person 4")).toHaveCount(1);
  await app.press("g", "i");
  await app.settle();

  const now = await app.now();
  await app.focus(false);
  await gmail.modify(A, m("p2"), [], ["STARRED"]);
  await app.focus(true);
  await app.settle();
  expect(await polls()).toBe(before + 2);
  expect(await app.now()).toBe(now);
  await expect(app.row("Person 2")).not.toHaveAttribute("data-starred");
});

test("SYN-04 history too old: a full resync that sends no notifications", async ({
  app,
  gmail,
  page,
}) => {
  await page.setViewportSize({ width: 1280, height: 2000 });
  const s = await seeds.standard(gmail, app);

  await app.highlight("Person 6", { settle: true });
  await app.pick("h");
  await expect(app.row("Person 6")).toHaveCount(0);
  await app.settle();

  await gmail.expireHistory(A);
  await gmail.deliverMany(
    Array.from({ length: 5 }, (_, i) => ({
      account: A,
      from: `Fay Five <fay${i + 1}@five.test>`,
      subject: `Missed ${i + 1}`,
      text: `Missed while history expired ${i + 1}.`,
      labels: ["INBOX", "UNREAD", "CATEGORY_PERSONAL"],
      poll: false,
    })),
  );
  await gmail.deleteThread(A, s.thread("p4").threadId);
  await gmail.modify(A, s.message("p5", 1).id, [], ["INBOX"]);
  await gmail.modify(A, s.message("p6", 1).id, ["INBOX"], []);
  await gmail.clearRecorded();
  const since = (await gmail.requests()).at(-1)?.seq ?? -1;

  await gmail.poll();
  await app.settle();

  const log = (await gmail.requests()).filter((r) => r.seq > since && r.account === A);
  const profile = log.findIndex((r) => r.api === "getProfile");
  const listing = log.findIndex(
    (r) => r.api === "threads.list" && r.params.includeSpamTrash === "true",
  );
  expect(log.find((r) => r.api === "history.list")?.answer).toBe(404);
  expect(profile).toBeGreaterThanOrEqual(0);
  expect(listing).toBeGreaterThan(profile);
  expect(log[listing]?.params.maxResults).toBe("500");

  await expectInboxMatchesGmail(app, A);
  await expect(app.row("Person 4")).toHaveCount(0);
  const p4 = s.thread("p4").threadId;
  const minimal = log.filter(
    (r) => r.api === "threads.get" && r.params.format === "minimal" && r.path.endsWith(p4),
  );
  expect(minimal.map((r) => r.answer)).toEqual([404]);
  await expect(app.row("Person 5")).toHaveCount(0);
  await expect(app.row("Person 6")).toHaveCount(1);
  await app.press("g", "h");
  await app.settle();
  await expect(app.rows).toHaveCount(0);
  await app.press("g", "i");
  await app.settle();
  expect((await gmail.platform()).notifications).toEqual([]);

  const next = await gmail.deliver({
    account: A,
    from: "Pat Poll <pat@poll.test>",
    subject: "After the resync",
    text: "Polling works again.",
    labels: ["INBOX", "UNREAD", "CATEGORY_PERSONAL"],
  });
  await app.settle();
  await expect(app.row("After the resync")).toHaveCount(1);
  expect((await gmail.platform()).notifications).toEqual([
    expect.objectContaining({ account: A, thread: next.threadId }),
  ]);
});

test("SYN-05 the full resync keeps a thread that moved during the listing", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  const p2 = s.thread("p2").threadId;
  await gmail.expireHistory(A);
  await gmail.fault(
    { api: "threads.list", query: { includeSpamTrash: "true" } },
    { kind: "omitThreads", threads: [p2] },
  );
  await gmail.poll();
  await app.settle();

  const resync = (await gmail.requests("threads.list", { account: A })).filter(
    (r) => r.params.includeSpamTrash === "true",
  );
  expect(resync.length).toBeGreaterThan(0);
  const check = (await gmail.requests("threads.get", { account: A })).filter(
    (r) => r.params.format === "minimal" && r.path.endsWith(p2),
  );
  expect(check.map((r) => r.answer)).toEqual([200]);
  await expect(app.row("Person 2")).toHaveCount(1);
});

test("SYN-06 sendAs and labels refresh on focus", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  await gmail.sendAs(A, "ana@new.dev", "Ana New");
  await gmail.label(A, "Fresh");
  await app.focus(false);
  await app.focus(true);
  await app.settle();

  await app.compose();
  await app.press("Meta+Shift+F");
  await expect(app.palette).toBeVisible();
  await expect(app.palette.locator("[cmdk-item]").filter({ hasText: "ana@new.dev" })).toHaveCount(
    1,
  );
  await app.press("Escape");
  await app.press("Escape", "Escape");
  await expect(app.page.getByRole("heading", { name: "New Message" })).toHaveCount(0);

  await app.press("l");
  await expect(app.palette).toBeVisible();
  await expect(app.palette.locator("[cmdk-item]").filter({ hasText: "Fresh" })).toHaveCount(1);
  await app.press("Escape");
});

test("SYN-07 an opened thread fetches its bodies right away", async ({ app, gmail, page }) => {
  test.slow();
  await page.setViewportSize({ width: 1280, height: 2600 });
  await gmail.quota(true);
  await seeds.bulk(gmail, app, { open: false });
  await gmail.start();
  const now = await gmail.now();
  await page.clock.install({ time: now });
  await page.clock.setFixedTime(now);
  await page.goto(gmail.url);
  await expect(page.getByRole("navigation")).toBeVisible();

  const bodies = async () =>
    (await gmail.requests("threads.get", { account: A })).filter((r) => r.params.format === "full");
  const row = app.row("Offer 10");
  await expect(row).toHaveCount(1, { timeout: 60_000 });
  const before = await bodies();
  expect(before.length).toBeLessThan(50);
  await row.click();
  await expect(app.conversationTitle).toHaveText("Offer 10");
  await expect(
    app.conversation.locator("iframe[data-frame]").last().contentFrame().locator("body"),
  ).toContainText("Promotion 10.", { timeout: 5_000 });
  expect((await bodies()).filter((r) => r.answer === 200).length).toBeLessThan(60);
});

test("SYN-08 a slower thread fetch never undoes a newer one", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  const held = async () => (await gmail.requests("threads.get")).some((r) => r.held);
  const bodyFetch = { api: "threads.get", query: { format: "full" } } as const;
  const from = seeds.PERSONS[0] ?? "";

  let slow = await gmail.holdAnswer(bodyFetch, { times: 1 });
  const first = await gmail.deliver({ account: A, from, subject: "Race", text: "First." });
  await expect.poll(held).toBe(true);
  await gmail.deliver({
    account: A,
    from,
    subject: "Re: Race",
    text: "Second.",
    threadId: first.threadId,
    inReplyTo: first.messageId,
  });
  await app.settle();
  await slow.release();
  await app.settle();
  await app.row("Race").click();
  await expect(app.conversationTitle).toHaveText("Race");
  await expect(app.messages, "the stale answer deletes nothing").toHaveCount(2);
  await app.press("Escape");
  await app.settle();

  slow = await gmail.holdAnswer(bodyFetch, { times: 1 });
  const other = await gmail.deliver({ account: A, from, subject: "Late read", text: "Hi." });
  await expect.poll(held).toBe(true);
  await gmail.modify(A, other.id, [], ["UNREAD"], { poll: true });
  await app.settle();
  await expect(app.row("Late read")).not.toHaveAttribute("data-unread");
  await slow.release();
  await app.settle();
  await expect(app.row("Late read"), "the stale answer keeps it read").not.toHaveAttribute(
    "data-unread",
  );
});

test("SYN-09 a connect sync retried after a failure misses no change made in between", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app, { open: false });
  await gmail.fault("drafts.list", { kind: "status", status: 429 }, { account: A, times: 1 });
  await app.open();
  await expect(app.row("Person 1")).toHaveCount(1);

  await gmail.modify(A, s.message("p1", 1).id, [], ["INBOX"]);
  await app.advance(10_000);
  await app.settle();
  await expect(app.row("Person 1")).toHaveCount(0);
  await expectInboxMatchesGmail(app, A);
});

test("SYN-12 a quit during the first sync loses none of it: starred and recent sent mail are read after the relaunch", async ({
  app,
  gmail,
}) => {
  await seeds.drafts(gmail, app, { open: false });
  const starred = await gmail.hold("threads.list", { account: A, query: { labelIds: "STARRED" } });
  const sent = await gmail.hold("threads.list", {
    account: A,
    query: { q: "in:sent newer_than:90d" },
  });
  await app.open();
  await expect(app.row("Person 1")).toHaveCount(1);
  await app.settle();
  await expect
    .poll(
      async () =>
        (await gmail.requests("threads.list", { account: A })).filter((r) => r.held).length,
    )
    .toBe(2);

  await app.restart(async () => {
    await gmail.release({ id: starred.id });
    await gmail.release({ id: sent.id });
  });
  await expect(app.row("Person 1")).toHaveCount(1);
  await app.settle();

  await app.press("g", "s");
  await app.settle();
  await expect(app.row("Trip ideas")).toHaveCount(1);
  await app.press("g", "t");
  await app.settle();
  await expect(app.row("Sent 10 days ago")).toHaveCount(1);
  await expect(app.row("Sent 89 days ago")).toHaveCount(1);
});

test("SYN-14 a quit while the first sync still reads starred and sent mail leaves it unfinished: the relaunch reads them", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app, { open: false });
  const trip = s.thread("starredOld").threadId;
  const read = await gmail.hold({ api: "threads.get", path: `*/threads/${trip}`, account: A });
  await app.open();
  await expect(app.row("Person 1")).toHaveCount(1);
  await app.settle();
  await expect
    .poll(async () => (await gmail.requests("threads.get", { account: A })).some((r) => r.held))
    .toBe(true);

  await app.restart(async () => {
    await gmail.release({ id: read.id });
  });
  await expect(app.row("Person 1")).toHaveCount(1);
  await app.settle();

  await app.press("g", "s");
  await app.settle();
  await expect(app.row("Trip ideas")).toHaveCount(1);
});

test("SYN-13 a quit during the contacts backfill does not end it: it runs again after the relaunch", async ({
  app,
  gmail,
}) => {
  test.fail(
    true,
    "BUG: the backfill is queued in memory by the connect sync only, so a quit ends it for good. Resuming it needs a stored mark on the account: a new column, which goes in with the first schema migration.",
  );
  const backfills = async () =>
    (await gmail.requests("threads.list", { account: A })).filter(
      (r) => r.params.q === "in:sent newer_than:1y",
    );
  await seeds.standard(gmail, app, { open: false });
  const backfill = await gmail.hold("threads.list", {
    account: A,
    query: { q: "in:sent newer_than:1y" },
  });
  await app.open();
  await expect(app.row("Person 1")).toHaveCount(1);
  await app.settle();
  await expect.poll(async () => (await backfills()).some((r) => r.held)).toBe(true);
  const before = (await backfills()).length;

  await app.restart(async () => {
    await gmail.release({ id: backfill.id });
  });
  await expect(app.row("Person 1")).toHaveCount(1);
  await app.settle();
  expect((await backfills()).length).toBeGreaterThan(before);
});

test("SYN-11 a slower label or sendAs read never undoes a newer one", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  const refresh = async () => {
    await app.focus(false);
    await app.focus(true);
  };
  const held = async (api: "labels.list" | "sendAs.list") =>
    (await gmail.requests(api, { account: A })).some((r) => r.held);
  const fromPicker = async () => {
    await app.compose();
    await app.press("Meta+Shift+F");
    await expect(app.palette).toBeVisible();
    const aliases = app.palette.locator("[cmdk-item]").filter({ hasText: "ana@new.dev" });
    const count = await aliases.count();
    await app.press("Escape");
    await app.press("Escape", "Escape");
    await expect(app.page.getByRole("heading", { name: "New Message" })).toHaveCount(0);
    return count;
  };

  const list = await gmail.holdAnswer("labels.list", { account: A, times: 1 });
  await refresh();
  await expect.poll(() => held("labels.list")).toBe(true);
  await gmail.label(A, "Newer");
  await refresh();
  await app.settle();
  await list.release();
  await app.settle();
  await app.press("g", "l");
  await expect(
    app.palette.getByRole("option", { name: "Newer", exact: true }),
    "the stale list deletes nothing",
  ).toHaveCount(1);
  await app.press("Escape");
  await expect(app.palette).toHaveCount(0);

  const sendAs = await gmail.holdAnswer("sendAs.list", { account: A, times: 1 });
  await refresh();
  await expect.poll(() => held("sendAs.list")).toBe(true);
  await gmail.sendAs(A, "ana@new.dev", "Ana New");
  await refresh();
  await app.settle();
  await sendAs.release();
  await app.settle();
  expect(await fromPicker(), "the stale sendAs drops no alias").toBe(1);
});
