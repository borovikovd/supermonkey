import type { App } from "./app.ts";
import type { Control, Delivery } from "./control.ts";
import { expect, test } from "./fixtures.ts";
import * as seeds from "./seeds.ts";
import { A } from "./seeds.ts";

const NEWS = "News Co <news@news.test>";

/** A newsletter from News Co with these list headers. */
function newsletter(subject: string, headers: [string, string][]): Delivery {
  return { account: A, from: NEWS, subject, text: `${subject} body.`, headers };
}

/** Opens the ⌘U menu on the highlighted row and runs the choice that matches `choice`. */
async function unsubscribe(app: App, choice: string): Promise<void> {
  await app.press("Meta+u");
  await expect(app.palette).toBeVisible();
  await app.type(choice);
  await expect(app.palette.locator("[cmdk-item]").first()).toContainText(choice);
  await app.press("Enter");
  await expect(app.palette).toHaveCount(0);
}

/** Moves the highlight onto the row containing `text` with j / k. */
async function highlight(app: App, text: string): Promise<void> {
  const { at, to } = await app.rows.evaluateAll(
    (rows, t) => ({
      at: rows.findIndex((r) => r.getAttribute("aria-selected") === "true"),
      to: rows.findIndex((r) => (r.textContent ?? "").includes(t)),
    }),
    text,
  );
  if (to < 0) throw new Error(`no row with ${text}`);
  const delta = to - at;
  await app.press(...Array.from({ length: Math.abs(delta) }, () => (delta > 0 ? "j" : "k")));
  await expect(app.highlighted).toContainText(text);
}

async function labelsOf(gmail: Control, id: string): Promise<string[]> {
  return (await gmail.gmail(A)).messages.find((m) => m.id === id)?.labels ?? [];
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
  // One click is enough: no email to the list, and the mail stays where it is.
  expect(await gmail.requests("messages.send")).toEqual([]);
  await app.settle();
  expect(await labelsOf(gmail, news.id)).toContain("INBOX");
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
  await highlight(app, "News two");

  await unsubscribe(app, "Unsubscribe, and mark done all");
  await expect(app.row("News one")).toHaveCount(0);
  await expect(app.row("News two")).toHaveCount(0);
  await expect(app.page.getByText("Unsubscribed from News Co")).toBeVisible();
  await app.settle();

  const sent = (await gmail.gmail(A)).messages.filter((m) => m.labels.includes("SENT"));
  expect(sent).toHaveLength(1);
  expect(sent[0]?.headers.to).toContain("leave@news.test");
  expect(sent[0]?.headers.subject).toBe("unsubscribe-me");
  expect(await labelsOf(gmail, older?.id ?? "")).not.toContain("INBOX");
  expect(await labelsOf(gmail, newer?.id ?? "")).not.toContain("INBOX");
  expect(await labelsOf(gmail, other?.id ?? "")).toContain("INBOX");

  // The archive is an ordinary action: z brings both back.
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
  expect(await labelsOf(gmail, one?.id ?? "")).toContain("TRASH");
  expect(await labelsOf(gmail, two?.id ?? "")).toContain("TRASH");
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
  expect(await labelsOf(gmail, plain.id)).toContain("INBOX");
  expect(await gmail.requests("messages.send")).toEqual([]);

  // Offline: the local archive goes ahead, and the unsubscribe reports it failed.
  await gmail.offline(true);
  await unsubscribe(app, "Unsubscribe, and mark done all");
  await expect(app.row("No link here")).toHaveCount(0);
  await expect(app.page.getByText(/Couldn't unsubscribe/)).toBeVisible();
});
