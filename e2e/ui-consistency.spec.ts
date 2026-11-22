import type { Route } from "@playwright/test";
import type { App } from "./app.ts";
import { expect, test } from "./fixtures.ts";
import * as seeds from "./seeds.ts";
import { A } from "./seeds.ts";

const two = (i: number) => String(i).padStart(2, "0");

const pageAck = (app: App): Promise<number> =>
  app.page.evaluate<number>("window.__sm.pending().ack");

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
  const afterFirstFive = (await probe.added()).length;
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
  await gmail.modify(A, s.latest("important3").id, ["STARRED"], []);
  await gmail.poll();
  await app.press("e", "e", "e", "e", "e");
  await expect(app.row("Bulk 10")).toHaveCount(0);
  const afterAll = (await probe.added()).length;
  await hold.release();
  await app.settle();

  const added = await probe.added();
  const firstFive = new Set(archived.slice(0, 5));
  expect(added.slice(afterFirstFive).filter((k) => firstFive.has(k))).toEqual([]);
  expect(added.slice(afterAll).filter((k) => archived.includes(k))).toEqual([]);

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
  const archivedThreads = Array.from(
    { length: 10 },
    (_, i) => s.thread(`important${i + 1}`).threadId,
  );
  expect(archivedThreads.filter((t) => inbox.has(t))).toEqual([]);

  await expect(app.highlighted).toContainText("Bulk 11");
});

test("UIC-08 an action stays applied when the one before it is acked", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  await expect(app.highlighted).toContainText("Person 1");
  const p2 = await app.rowKey("Person 2");
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

  await toBottom();
  await expect(app.row("Batch 90")).toHaveAttribute("data-unread");

  await gmail.modify(A, last.id, [], ["INBOX"], { poll: true });
  await app.settle();
  await expect(app.row("Batch 90")).toHaveCount(0);
  await gmail.modify(A, last.id, [], ["UNREAD"], { poll: true });
  await app.settle();

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
  const parked: Route[] = [];
  const submit = /\/invoke\/submit$/;
  await app.page.route(submit, (route) => {
    parked.push(route);
  });
  await app.page.clock.pauseAt(await app.now());

  const quitting = app.quit();
  await expect.poll(() => parked.length).toBe(1);
  await app.page.clock.runFor(2_000);
  const owed = await app.page.evaluate<number>("window.__sm.pending().invokes");
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
    app.errorToasts.filter({ hasText: "That account is no longer in Supermonkey" }),
  ).toHaveCount(1);
  await expect(mute).toHaveAttribute("aria-checked", "false");
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
  const bug = app.errorToasts.filter({ hasText: "because of a bug in Supermonkey" });
  await expect(app.highlighted).toContainText("Person 1");
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
