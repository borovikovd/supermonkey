// Rows never flash back during fast triage or a poll, and a reload keeps
// queued actions and their numbers.
import type { Route } from "@playwright/test";
import type { App } from "./app.ts";
import { expect, test } from "./fixtures.ts";
import * as seeds from "./seeds.ts";
import { A } from "./seeds.ts";

const two = (i: number) => String(i).padStart(2, "0");

/** The last action number the page has seen acked on the channel. */
const pageAck = (app: App): Promise<number> =>
  app.page.evaluate<number>("window.__ub.pending().ack");

test("UIC-01 rows you just acted on never flash back during fast triage or a poll", async ({
  app,
  gmail,
}) => {
  const s = await seeds.bulk(gmail, app);
  await expect(app.highlighted).toContainText("Bulk 01");
  const archived = await Promise.all(
    Array.from({ length: 10 }, (_, i) => app.rowKey(`Bulk ${two(i + 1)}`)),
  );
  const probe = await app.watchRows();
  const hold = await gmail.hold("messages.batchModify");

  await app.press("e", "e", "e", "e", "e");
  await expect(app.row("Bulk 05")).toHaveCount(0);
  // A row React only moves is added again too: count adds from when each row left.
  const afterFirstFive = (await probe.added()).length;
  // Halfway: two messages arrive in other threads, and a poll runs.
  // Bulk 31's reply is the newer one, so it leads the list.
  await Promise.all(
    (
      [
        ["important30", 30_000],
        ["important31", 0],
      ] as const
    ).map(([key, ago]) =>
      gmail.deliver({
        account: A,
        from: "Late Reply <late@bulk.test>",
        subject: `Re: ${s.thread(key).subject}`,
        threadId: s.thread(key).threadId,
        inReplyTo: s.latest(key).messageId,
        date: seeds.ANCHOR - ago,
        poll: false,
      }),
    ),
  );
  // Another client stars Bulk 03 while its archive is held, so the poll reads
  // that thread with Gmail's INBOX still on it.
  await gmail.modify(A, s.latest("important3").id, ["STARRED"], []);
  await gmail.poll();
  await app.press("e", "e", "e", "e", "e");
  await expect(app.row("Bulk 10")).toHaveCount(0);
  const afterAll = (await probe.added()).length;
  await hold.release();
  await app.settle();

  // No archived row was ever added back.
  const added = await probe.added();
  const firstFive = new Set(archived.slice(0, 5));
  expect(added.slice(afterFirstFive).filter((k) => firstFive.has(k))).toEqual([]);
  expect(added.slice(afterAll).filter((k) => archived.includes(k))).toEqual([]);

  // The list equals Gmail's Inbox: Bulk 31 and 30 (newest now), the rest of Bulk 11–40, then Offer 01–20.
  const inbox = new Set(
    (await gmail.gmail(A)).messages
      .filter((m) => m.labels.includes("INBOX"))
      .map((m) => m.threadId),
  );
  expect(inbox.size).toBe(50);
  expect(await app.splitHeader()).toBe("Important 30 · Other 20");
  const expected = [
    "Bulk 31",
    "Bulk 30",
    ...Array.from({ length: 30 }, (_, i) => `Bulk ${two(i + 11)}`).filter(
      (x) => x !== "Bulk 30" && x !== "Bulk 31",
    ),
    ...Array.from({ length: 20 }, (_, i) => `Offer ${two(i + 1)}`),
  ];
  const rendered = (await app.sections()).flatMap((x) => x.subjects);
  expect(rendered.length).toBeGreaterThan(10);
  expect(rendered).toEqual(expected.slice(0, rendered.length));
  // Every Inbox thread in Gmail is one the app lists, and none it archived.
  const archivedThreads = Array.from(
    { length: 10 },
    (_, i) => s.thread(`important${i + 1}`).threadId,
  );
  expect(archivedThreads.filter((t) => inbox.has(t))).toEqual([]);

  // The highlight is on the row after the last archived one.
  await expect(app.highlighted).toContainText("Bulk 11");
});

test("UIC-01 an action the core has not applied yet stays applied when the one before it is acked", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  await expect(app.highlighted).toContainText("Person 1");
  const p2 = await app.rowKey("Person 2");
  // The page's submits wait here, so they reach the core one at a time.
  const parked: Route[] = [];
  await app.page.route(/\/invoke\/submit$/, (route) => {
    parked.push(route);
  });
  await app.press("e");
  await expect.poll(() => parked.length).toBe(1);
  await app.press("e");
  await expect(app.row("Person 1")).toHaveCount(0);
  await expect(app.row("Person 2")).toHaveCount(0);
  const probe = await app.watchRows();
  const before = await pageAck(app);

  // The core applies the first archive and acks it; the second is submitted and waits.
  await parked[0]?.continue();
  await expect.poll(() => parked.length).toBe(2);
  await expect.poll(() => pageAck(app)).toBeGreaterThan(before);
  await expect(app.row("Person 2"), "still archived while the core lacks it").toHaveCount(0);

  await parked[1]?.continue();
  await app.page.unroute(/\/invoke\/submit$/);
  await app.settle();
  await expect(app.row("Person 2")).toHaveCount(0);
  expect(await probe.added(), "Person 2 never came back").not.toContain(p2);
});

test("UIC-02 a reload keeps queued actions and keeps action numbers going", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  const inInbox = async (key: string) =>
    (await gmail.gmail(A)).messages
      .filter((m) => m.threadId === s.thread(key).threadId)
      .some((m) => m.labels.includes("INBOX"));

  await gmail.offline(true);
  await expect(app.highlighted).toContainText("Person 1");
  await app.press("e");
  await expect(app.row("Person 1")).toHaveCount(0);

  await app.reload();
  await expect(app.row("Person 1")).toHaveCount(0);
  await expect(app.highlighted).toContainText("Person 2");
  await app.press("e");
  await expect(app.row("Person 2")).toHaveCount(0);

  await gmail.offline(false);
  await app.settle();
  expect(await inInbox("p1")).toBe(false);
  expect(await inInbox("p2")).toBe(false);

  // Both entries survived the reload: z undoes p2, then p1.
  await app.press("z");
  await expect(app.row("Person 2")).toHaveCount(1);
  await expect(app.row("Person 1")).toHaveCount(0);
  await app.settle();
  expect(await inInbox("p2")).toBe(true);
  expect(await inInbox("p1")).toBe(false);
  await app.press("z");
  await expect(app.row("Person 1")).toHaveCount(1);
  await app.settle();
  expect(await inInbox("p1")).toBe(true);
});

test("UIC-03 an action whose submit fails is sent again", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  const inInbox = async (key: string) =>
    (await gmail.gmail(A)).messages
      .filter((m) => m.threadId === s.thread(key).threadId)
      .some((m) => m.labels.includes("INBOX"));
  await expect(app.highlighted).toContainText("Person 1");
  // The first submit fails before it reaches the core; later ones go through.
  let failed = 0;
  const submit = /\/invoke\/submit$/;
  await app.page.route(submit, async (route) => {
    if (failed > 0) return route.continue();
    failed++;
    return route.fulfill({ status: 500, json: "the bridge is down" });
  });

  await app.press("e");
  await expect(app.row("Person 1")).toHaveCount(0);
  await expect.poll(() => failed).toBe(1);
  await app.settle();
  await app.page.unroute(submit);

  expect(await inInbox("p1"), "the archive reached Gmail").toBe(false);
  await expect(app.row("Person 1")).toHaveCount(0);
  await app.reload();
  await expect(app.row("Person 1")).toHaveCount(0);
});

test("UIC-04 a row that changed while no list held it is not shown from an old copy", async ({
  app,
  gmail,
}) => {
  await seeds.empty(gmail, app);
  // 90 threads: the oldest is past the rows a first load carries.
  const sent = await gmail.deliverMany(
    Array.from({ length: 90 }, (_, i) => ({
      account: A,
      from: `Writer ${two(i + 1)} <writer${i + 1}@rows.test>`,
      subject: `Batch ${two(i + 1)}`,
      date: seeds.dates.minutesAgo(i + 1),
      poll: false,
    })),
  );
  await gmail.poll();
  await app.settle();
  const last = sent.at(-1);
  if (!last) throw new Error("nothing delivered");
  const scroller = app.list.locator("xpath=..");
  const toBottom = () =>
    scroller.evaluate((el) => {
      el.scrollTop = el.scrollHeight;
    });

  // The page has Batch 90's row, unread.
  await toBottom();
  await expect(app.row("Batch 90")).toHaveAttribute("data-unread");

  // Another client archives it, then marks it read: no open list holds it by then.
  await gmail.modify(A, last.id, [], ["INBOX"], { poll: true });
  await app.settle();
  await expect(app.row("Batch 90")).toHaveCount(0);
  await gmail.modify(A, last.id, [], ["UNREAD"], { poll: true });
  await app.settle();

  // A search lists it again, past its first rows: it shows as read.
  await app.press("/");
  await app.page.getByPlaceholder("Search").fill("Batch");
  await app.settle();
  await expect(app.rows.first()).toBeVisible();
  await toBottom();
  await expect(app.row("Batch 90")).toBeVisible();
  await app.settle();
  await expect(app.row("Batch 90")).not.toHaveAttribute("data-unread");
});

test("UIC-05 Cmd+Q quits only once the composer's close has committed", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  await app.press("j", "j", "Enter");
  await app.reply();
  await app.type("Written just before the quit");
  // The submit carrying the close waits here; page timers wait for the test.
  const parked: Route[] = [];
  const submit = /\/invoke\/submit$/;
  await app.page.route(submit, (route) => {
    parked.push(route);
  });
  await app.page.clock.pauseAt(await app.now());

  const quitting = app.quit();
  await expect.poll(() => parked.length).toBe(1);
  await app.page.clock.runFor(2_000);
  const owed = await app.page.evaluate<number>("window.__ub.pending().invokes");
  expect(owed, "only the close is in flight: the page has not asked to quit").toBe(1);

  await Promise.all(parked.map((r) => r.continue()));
  await app.page.unroute(submit);
  await app.page.clock.resume();
  await quitting;
  await app.launch();
  const drafts = (await gmail.gmail(A)).drafts;
  expect(drafts).toHaveLength(1);
  expect(drafts[0]?.message.raw).toContain("Written just before the quit");
});

test("UIC-06 an action the core rejects tells the user why", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  // The mute names an account the core doesn't have, as a UI out of step
  // with the core would.
  const submit = /\/invoke\/submit$/;
  await app.page.route(submit, (route) =>
    route.continue({
      postData: (route.request().postData() ?? "").replace(
        /"setting":"muted","account":\d+/,
        '"setting":"muted","account":99',
      ),
    }),
  );
  await app.press("Meta+,");
  const mute = app.sheet("Settings").getByRole("switch", { name: "Mute notifications" }).first();
  await mute.click();
  await app.settle();
  await app.page.unroute(submit);

  await expect(
    app.errorToasts.filter({ hasText: "That account is no longer in Ubermensch" }),
  ).toHaveCount(1);
  // The switch falls back to what the core holds.
  await expect(mute).toHaveAttribute("aria-checked", "false");
  // Shown once: the refusal came with the message that settled the action,
  // so a reload does not bring it back.
  await app.reload();
  await app.expectNoErrorToasts();
});

test("UIC-07 an action the core can't read is refused, and the actions after it still apply", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  const labels = async (key: string) =>
    (await gmail.gmail(A)).messages
      .filter((m) => m.threadId === s.thread(key).threadId)
      .flatMap((m) => m.labels);
  const bug = app.errorToasts.filter({ hasText: "because of a bug in Ubermensch" });
  await expect(app.highlighted).toContainText("Person 1");
  // Every submit carries the star with a thread the core can't read, as a
  // page out of step with the core would send it.
  const submit = /\/invoke\/submit$/;
  await app.page.route(submit, (route) =>
    route.continue({
      postData: (route.request().postData() ?? "").replace(
        '"kind":"star","threads":[',
        '"kind":"star","threads":[null,',
      ),
    }),
  );
  await app.press("s");
  await app.press("e");
  await expect(app.row("Person 1")).toHaveCount(0);
  await app.settle();
  expect(await labels("p1"), "the archive after it reached Gmail").not.toContain("INBOX");
  expect(await labels("p1")).not.toContain("STARRED");
  await expect(bug).toHaveCount(1);

  // A batch the core can't read at all, here an action with no number, is
  // not sent again: its actions are refused, and the next one goes through.
  await app.page.unroute(submit);
  await app.page.route(submit, (route) =>
    route.continue({
      postData: (route.request().postData() ?? "").replace(
        /"n":\d+(,"action":\{"type":"modify","account":\d+,"kind":"(?:un)?read")/,
        '"n":null$1',
      ),
    }),
  );
  await expect(app.highlighted).toContainText("Person 2");
  await expect(app.row("Person 2")).not.toHaveAttribute("data-unread");
  await app.press("u");
  await expect(bug).toHaveCount(2);
  await expect(app.row("Person 2")).not.toHaveAttribute("data-unread");
  await app.press("e");
  await expect(app.row("Person 2")).toHaveCount(0);
  await app.settle();
  expect(await labels("p2")).not.toContain("INBOX");
  expect(await labels("p2")).not.toContain("UNREAD");
});
