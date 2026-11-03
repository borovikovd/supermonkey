// Rows never flash back during fast triage or a poll, and a reload keeps
// queued actions and their numbers (COVERAGE 5.24).
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
