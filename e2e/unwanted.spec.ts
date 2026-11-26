import type { App } from "./app.ts";
import type { Delivery } from "./control.ts";
import { expect, test } from "./fixtures.ts";
import * as seeds from "./seeds.ts";
import { A, B } from "./seeds.ts";

const NEWS = "News Co <news@news.test>";

function newsletter(subject: string, headers: [string, string][]): Delivery {
  return { account: A, from: NEWS, subject, text: `${subject} body.`, headers };
}

async function unsubscribe(app: App, choice: string): Promise<void> {
  await app.press("Meta+u");
  await expect(app.palette).toBeVisible();
  await app.type(choice);
  await expect(app.palette.locator("[cmdk-item]").first()).toContainText(choice);
  await app.press("Enter");
  await expect(app.palette).toHaveCount(0);
}

async function choose(app: App, name: string, choice: string): Promise<void> {
  await app.press("Meta+k");
  await expect(app.palette).toBeVisible();
  await app.type(name);
  await expect(app.palette.locator("[cmdk-item]").first()).toContainText(name);
  await app.press("Enter");
  await app.type(choice);
  await expect(app.palette.locator("[cmdk-item]").first()).toContainText(choice);
  await app.press("Enter");
  await expect(app.palette).toHaveCount(0);
}

test("UNW-01 Cmd+U unsubscribes with one click when the list offers it", async ({ app, gmail }) => {
  await seeds.empty(gmail, app);
  const news = await gmail.deliver(
    newsletter("Weekly news", [
      ["List-Unsubscribe", `<mailto:leave@news.test>, <${gmail.url}/sink/unsub-1>`],
      ["List-Unsubscribe-Post", "List-Unsubscribe=One-Click"],
    ]),
  );
  await app.settle();
  await expect(app.highlighted).toContainText("Weekly news");

  await unsubscribe(app, "Unsubscribe");
  await expect(app.page.getByText("Unsubscribed from News Co")).toBeVisible();
  const posts = (await gmail.hits()).filter((h) => h.path === "/sink/unsub-1");
  expect(posts.map((h) => [h.method, h.body])).toEqual([["POST", "List-Unsubscribe=One-Click"]]);
  expect(await gmail.requests("messages.send")).toEqual([]);
  await app.settle();
  expect(await gmail.labelsOf(A, news.id)).toContain("INBOX");
  await expect(app.row("Weekly news")).toHaveCount(1);
});

test("UNW-02 an email unsubscribes, and mark done all archives the sender's Inbox mail", async ({
  app,
  gmail,
}) => {
  await seeds.empty(gmail, app);
  const header: [string, string][] = [
    ["List-Unsubscribe", "<mailto:leave@news.test?subject=unsubscribe-me>"],
  ];
  const [older, newer, other] = await gmail.deliverMany([
    newsletter("News one", header),
    newsletter("News two", header),
    { account: A, from: "Pat Person <pat@corp.test>", subject: "Lunch?" },
  ]);
  await app.settle();
  await app.highlight("News two");

  await unsubscribe(app, "Unsubscribe, and mark done all");
  await expect(app.row("News one")).toHaveCount(0);
  await expect(app.row("News two")).toHaveCount(0);
  await expect(app.page.getByText("Unsubscribed from News Co")).toBeVisible();
  await app.settle();

  const sent = (await gmail.gmail(A)).messages.filter((m) => m.labels.includes("SENT"));
  expect(sent).toHaveLength(1);
  expect(sent[0]?.headers.to).toContain("leave@news.test");
  expect(sent[0]?.headers.subject).toBe("unsubscribe-me");
  expect(await gmail.labelsOf(A, older?.id ?? "")).not.toContain("INBOX");
  expect(await gmail.labelsOf(A, newer?.id ?? "")).not.toContain("INBOX");
  expect(await gmail.labelsOf(A, other?.id ?? "")).toContain("INBOX");

  await app.press("z");
  await expect(app.row("News one")).toHaveCount(1);
  await expect(app.row("News two")).toHaveCount(1);
});

test("UNW-03 a list with only a web page opens it, and trash all trashes the sender's mail", async ({
  app,
  gmail,
}) => {
  await seeds.empty(gmail, app);
  const page = "https://news.test/unsubscribe?u=42";
  const [one, two] = await gmail.deliverMany([
    newsletter("Deal one", [["List-Unsubscribe", `<${page}>`]]),
    newsletter("Deal two", [["List-Unsubscribe", `<${page}>`]]),
  ]);
  await app.settle();
  await expect(app.highlighted).toContainText(/Deal (one|two)/);

  await unsubscribe(app, "Unsubscribe, and trash all");
  await expect(app.page.getByText("Opened the unsubscribe page of News Co")).toBeVisible();
  expect((await gmail.platform()).openedUrls).toEqual([page]);
  await app.settle();
  expect(await gmail.labelsOf(A, one?.id ?? "")).toContain("TRASH");
  expect(await gmail.labelsOf(A, two?.id ?? "")).toContain("TRASH");
  expect(await gmail.requests("messages.send")).toEqual([]);
});

test("UNW-04 no unsubscribe link, or no network, says so; the archive still happens", async ({
  app,
  gmail,
}) => {
  await seeds.empty(gmail, app);
  const plain = await gmail.deliver({ account: A, from: NEWS, subject: "No link here" });
  await app.settle();
  await unsubscribe(app, "Unsubscribe");
  await expect(app.page.getByText("No unsubscribe link in this email")).toBeVisible();
  await app.settle();
  expect(await gmail.labelsOf(A, plain.id)).toContain("INBOX");
  expect(await gmail.requests("messages.send")).toEqual([]);

  await gmail.offline(true);
  await unsubscribe(app, "Unsubscribe, and mark done all");
  await expect(app.row("No link here")).toHaveCount(0);
  await expect(app.page.getByText(/Couldn't unsubscribe/)).toBeVisible();
});

test("UNW-05 Block makes a Gmail filter to Trash, trashes the conversation, and keeps new mail out", async ({
  app,
  gmail,
}) => {
  await seeds.empty(gmail, app);
  const promo = await gmail.deliver({ account: A, from: NEWS, subject: "Big sale" });
  await app.settle();
  await choose(app, "Block", "Block news@news.test");
  await expect(app.page.getByText("Blocked news@news.test")).toBeVisible();
  await expect(app.row("Big sale")).toHaveCount(0);
  await app.settle();
  expect(await gmail.filters(A)).toEqual([
    { id: expect.any(String), from: "news@news.test", add: ["TRASH"], remove: ["INBOX"] },
  ]);
  expect(await gmail.labelsOf(A, promo.id)).toContain("TRASH");

  const next = await gmail.deliver({ account: A, from: NEWS, subject: "Another sale" });
  await app.settle();
  expect(await gmail.labelsOf(A, next.id)).toEqual(expect.arrayContaining(["TRASH"]));
  await expect(app.row("Another sale")).toHaveCount(0);

  await gmail.deliver({ account: A, from: "Deals <deals@shop.test>", subject: "Deal" });
  await app.settle();
  await expect(app.highlighted).toContainText("Deal");
  await choose(app, "Block", "Block everyone at shop.test");
  await app.settle();
  expect((await gmail.filters(A)).map((f) => f.from)).toEqual(["news@news.test", "@shop.test"]);
});

test("UNW-06 Blocked senders lists the blocks, and Unblock deletes the filter", async ({
  app,
  gmail,
}) => {
  await seeds.empty(gmail, app);
  await gmail.deliver({ account: A, from: NEWS, subject: "Big sale" });
  await app.settle();
  await choose(app, "Block", "Block news@news.test");
  await app.settle();
  expect(await gmail.filters(A)).toHaveLength(1);

  await choose(app, "Blocked senders", "Unblock news@news.test");
  await expect(app.page.getByText("Unblocked news@news.test")).toBeVisible();
  await app.settle();
  expect(await gmail.filters(A)).toEqual([]);
  await app.press("Meta+k");
  await app.type("Blocked senders");
  await app.press("Enter");
  await expect(app.palette.getByPlaceholder("No blocked senders")).toBeVisible();
});

test("UNW-07 an account signed in before filters were asked for is offered Sign in again", async ({
  app,
  gmail,
}) => {
  await seeds.empty(gmail, app);
  await gmail.settingsScope(A, false);
  await gmail.deliver({ account: A, from: NEWS, subject: "Big sale" });
  await app.settle();
  await choose(app, "Block", "Block news@news.test");
  const toast = app.page.locator("[data-sonner-toast]", {
    hasText: "Blocking needs one more Google permission",
  });
  await expect(toast).toBeVisible();
  await expect(app.row("Big sale")).toHaveCount(1);
  expect(await gmail.filters(A)).toEqual([]);

  await gmail.nextSignIn({ email: A });
  await toast.getByRole("button", { name: "Sign in again" }).click();
  await app.settle();
  await choose(app, "Block", "Block news@news.test");
  await expect(app.page.getByText("Blocked news@news.test")).toBeVisible();
  await app.settle();
  expect((await gmail.filters(A)).map((f) => f.from)).toEqual(["news@news.test"]);
});

test("UNW-08 Mark spam, and block the domain", async ({ app, gmail }) => {
  await seeds.empty(gmail, app);
  const junk = await gmail.deliver({ account: A, from: "Spammer <x@junk.test>", subject: "Win" });
  await app.settle();
  await choose(app, "Mark as spam", "Mark spam, and block everyone at junk.test");
  await expect(app.row("Win")).toHaveCount(0);
  await expect(app.page.getByText("Blocked @junk.test")).toBeVisible();
  await app.settle();
  expect(await gmail.labelsOf(A, junk.id)).toContain("SPAM");
  expect(await gmail.labelsOf(A, junk.id)).not.toContain("TRASH");
  expect((await gmail.filters(A)).map((f) => f.from)).toEqual(["@junk.test"]);
});

test("UNW-09 unsubscribe, block, and unblock act only on the account's own thread and block filter", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  const invoke = (cmd: string, args: Record<string, unknown>) =>
    app.page.evaluate(
      async ([c, a]) => {
        const r = await fetch(`/invoke/${c}`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify(a),
        });
        return r.status;
      },
      [cmd, args] as const,
    );
  const [a, b] = [1, 2];
  const own = Number(await app.rowKey("Person 1"));
  await app.press("Control+2");
  const others = Number(await app.rowKey("B 2"));
  await app.press("Control+1");

  expect(await invoke("block", { account: a, thread: others, domain: false })).toBe(400);
  expect(await invoke("unsubscribe", { account: a, thread: others })).toBe(400);
  await app.settle();
  expect(await gmail.requests("filters.create")).toEqual([]);
  const bState = await gmail.gmail(B);
  expect(bState.messages.filter((m) => m.labels.includes("TRASH"))).toEqual([]);

  expect(await invoke("block", { account: a, thread: own, domain: false })).toBe(200);
  await app.settle();
  const [filter] = await gmail.filters(A);
  expect(filter?.add).toEqual(["TRASH"]);
  expect(await invoke("unblock", { account: a, id: "forged" })).toBe(400);
  expect(await invoke("unblock", { account: b, id: filter?.id })).toBe(400);
  expect(await gmail.requests("filters.delete")).toEqual([]);
  expect(await gmail.filters(A)).toHaveLength(1);
  expect(await invoke("unblock", { account: a, id: filter?.id })).toBe(200);
  expect(await gmail.filters(A)).toEqual([]);
});

test("UNW-10 block and unsubscribe act on their own account when another one is on screen by then", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  const trashedIn = async (email: string) =>
    (await gmail.gmail(email)).messages.filter((m) => m.labels.includes("TRASH")).length;
  const bTrash = await trashedIn(B);
  const promo = await gmail.deliver({
    account: A,
    from: "Pal <promo@pal.test>",
    subject: "Flash deal",
  });
  await app.settle();
  await app.highlight("Flash deal");

  const filter = await gmail.hold("filters.create");
  await choose(app, "Block", "Block promo@pal.test");
  await app.press("Control+2");
  await expect(app.highlighted).toContainText("Bea Blue");
  await filter.release();
  await expect(app.page.getByText("Blocked promo@pal.test")).toBeVisible();
  await app.settle();
  expect(await gmail.labelsOf(A, promo.id)).toContain("TRASH");
  expect(await trashedIn(B)).toBe(bTrash);
  await app.expectNoErrorToasts();
  await expect(app.highlighted).toContainText("Bea Blue");

  await app.press("Control+1");
  const header: [string, string][] = [["List-Unsubscribe", "<mailto:leave@news.test>"]];
  const [one, two] = await gmail.deliverMany([
    newsletter("News one", header),
    newsletter("News two", header),
  ]);
  await app.settle();
  await app.highlight("News two");
  const release = await app.park("sender_threads");
  await unsubscribe(app, "Unsubscribe, and mark done all");
  await app.press("Control+2");
  await expect(app.highlighted).toContainText("Bea Blue");
  await release();
  await app.settle();
  expect(await gmail.labelsOf(A, one?.id ?? "")).not.toContain("INBOX");
  expect(await gmail.labelsOf(A, two?.id ?? "")).not.toContain("INBOX");
  await app.expectNoErrorToasts();
  await expect(app.highlighted).toContainText("Bea Blue");
  await app.press("Control+1");
  await expect(app.row("News one")).toHaveCount(0);
  await expect(app.row("News two")).toHaveCount(0);
});

test("UNW-11 trash all after replying to the list leaves alone the conversations where you wrote last", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  const news = await gmail.deliver(
    newsletter("Weekly news", [["List-Unsubscribe", "<mailto:leave@news.test>"]]),
  );
  await app.settle();
  const replyTo = async (subject: string, text: string) => {
    await app.openThread(subject);
    await app.reply();
    await app.type(text);
    await app.send();
    await app.press("g", "i");
    await expect(app.row(subject)).toHaveCount(1);
  };
  await replyTo("Person 1", "I wrote last here");
  await replyTo("Weekly news", "Please stop");

  await app.highlight("Weekly news");
  await unsubscribe(app, "Unsubscribe, and trash all");
  await expect(app.page.getByText("Unsubscribed from News Co")).toBeVisible();
  await app.settle();

  expect(await gmail.labelsOf(A, news.id)).toContain("TRASH");
  const p1 = await gmail.labelsOf(A, s.message("p1", 1).id);
  expect(p1).toContain("INBOX");
  expect(p1).not.toContain("TRASH");
  await expect(app.row("Weekly news")).toHaveCount(0);
  await expect(app.row("Person 1")).toHaveCount(1);
  await app.expectNoErrorToasts();
});

test("UNW-12 blocking a sender who is already blocked trashes that conversation too", async ({
  app,
  gmail,
}) => {
  await seeds.empty(gmail, app);
  const [first, second] = await gmail.deliverMany([
    { account: A, from: NEWS, subject: "Sale one" },
    { account: A, from: NEWS, subject: "Sale two" },
  ]);
  await app.settle();
  await app.highlight("Sale one");
  await choose(app, "Block", "Block news@news.test");
  await expect(app.row("Sale one")).toHaveCount(0);
  await app.settle();
  await expect(app.row("Sale two")).toHaveCount(1);

  await app.highlight("Sale two");
  await choose(app, "Block", "Block news@news.test");
  await expect(app.row("Sale two")).toHaveCount(0);
  await app.settle();
  expect(await gmail.labelsOf(A, first?.id ?? "")).toContain("TRASH");
  expect(await gmail.labelsOf(A, second?.id ?? "")).toContain("TRASH");
  expect(await gmail.filters(A)).toHaveLength(1);
  await app.expectNoErrorToasts();
});
