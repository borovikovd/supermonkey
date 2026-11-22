import type { Locator } from "@playwright/test";

import type { App } from "./app.ts";
import type { Control } from "./control.ts";
import { expect, test } from "./fixtures.ts";
import * as seeds from "./seeds.ts";
import { A } from "./seeds.ts";

const viewHeader = (app: App): Locator => app.page.locator("div:has(> button[title='Search (/)'])");

const INBOX_HEADER = "Important 16 · Other 7";

async function expectHeader(app: App, name: string, message?: string): Promise<void> {
  if (name === INBOX_HEADER) {
    await expect.poll(() => app.splitHeader(), { message: message ?? name }).toBe(name);
  } else {
    await expect(viewHeader(app), message ?? name).toHaveText(name);
  }
}

async function inOrder<T>(items: T[], step: (item: T, i: number) => Promise<void>): Promise<void> {
  await items.reduce<Promise<void>>(
    (prev, item, i) => prev.then(() => step(item, i)),
    Promise.resolve(),
  );
}

const senderWeight = (row: Locator, sender: string) =>
  row.evaluate((el, name) => {
    const cell = Array.from(el.children).find((c) => c.textContent?.startsWith(name));
    if (!cell) throw new Error(`no ${name} in the row`);
    return Number(getComputedStyle(cell).fontWeight);
  }, sender);

async function repeat(app: App, key: string, n: number): Promise<void> {
  await app.press(...Array.from({ length: n }, () => key));
}

async function highlightInView(app: App): Promise<boolean> {
  return app.highlighted.evaluate((row) => {
    const scroller = row.closest("[role=listbox]")?.parentElement;
    if (!scroller) return false;
    const r = row.getBoundingClientRect();
    const s = scroller.getBoundingClientRect();
    return r.top >= s.top - 1 && r.bottom <= s.bottom + 1;
  });
}

async function expectActiveDescendant(app: App): Promise<void> {
  const id = await app.highlighted.getAttribute("id");
  expect(id).not.toBeNull();
  await expect(app.list).toHaveAttribute("aria-activedescendant", id ?? "");
}

const IMPORTANT = [
  "Person 1",
  "Person 2",
  "Person 3",
  "Person 4",
  "Person 5",
  "Person 6",
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

test("NAV-01 inbox rows show sender, subject — snippet, time, and an unread mark", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);

  expect(await app.sections()).toEqual([
    { name: "Important", subjects: IMPORTANT },
    { name: "Other", subjects: OTHER },
  ]);
  const inboxThreads = new Set(
    (await gmail.gmail(A)).messages
      .filter(
        (m) =>
          m.labels.includes("INBOX") && !m.labels.includes("TRASH") && !m.labels.includes("SPAM"),
      )
      .map((m) => m.threadId),
  );
  await expect(app.rows).toHaveCount(inboxThreads.size);

  const p1 = app.row("Person 1");
  await expect(p1).toContainText("Pia Park");
  await expect(p1).toContainText("Person 1 — Hello from Pia.");
  await expect(p1).toContainText(/\b9:50\sAM\b/);
  const plan = app.row("Q4 plan");
  await expect(plan).toContainText("Dave Diaz");
  await expect(plan).toContainText("Q4 plan — Plan message 12.");
  await expect(plan).toContainText(/\b8:00\sAM\b/);
  expect(s.thread("plan").messages).toHaveLength(12);
  const edges = await p1.evaluate((row) => {
    const last = row.lastElementChild;
    return {
      row: row.getBoundingClientRect().right,
      time: last ? last.getBoundingClientRect().right : 0,
    };
  });
  expect(edges.row - edges.time).toBeLessThanOrEqual(16);

  const unread = [
    "Q4 plan",
    "Fold check",
    "Offsite venue",
    "Person 1",
    "Person 3",
    "Person 5",
    "Digest",
  ];
  await expect(app.unread).toHaveCount(9);
  const unreadRows = (await Promise.all(unread.map((x) => app.row(x).all()))).flat();
  expect(unreadRows).toHaveLength(9);
  await Promise.all(unreadRows.map((row) => expect(row).toHaveAttribute("data-unread")));
  const read = app.row("Person 2");
  await expect(read).not.toHaveAttribute("data-unread");
  expect(await senderWeight(p1, "Pia Park")).toBeGreaterThan(await senderWeight(read, "Quin"));

  await expect(app.highlighted).toHaveCount(1);
  await expect(app.highlighted).toContainText("Person 1");

  expect(await app.splitHeader()).toBe("Important 16 · Other 7");
  await expect(app.page.getByText("E to Mark Done")).toBeVisible();
  await expect(app.page.getByText("H to snooze")).toBeVisible();
  await expect(app.page.getByText("⌘K for commands")).toBeVisible();
});

test("NAV-02 j and k move the highlight across split boundaries", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  await expect(app.highlighted).toContainText("Person 1");
  await repeat(app, "j", IMPORTANT.length - 1);
  await expect(app.highlighted).toContainText("Latest rule");
  await expectActiveDescendant(app);

  await app.press("j");
  await expect(app.highlighted).toContainText("Digest: week 44");
  await expect(app.highlighted).toHaveCount(1);
  await expectActiveDescendant(app);
  await expect(app.activeSplit).toHaveText("Other");

  await app.press("k");
  await expect(app.highlighted).toContainText("Latest rule");
  await expectActiveDescendant(app);
  await expect(app.activeSplit).toHaveText("Important");
});

test("NAV-03 Enter opens a conversation, Esc returns with the highlight; a click opens too", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  await repeat(app, "j", 3);
  await expect(app.highlighted).toContainText("Person 4");
  await app.press("Enter");

  await expect(app.conversationTitle).toHaveText("Person 4");
  await expect(app.list).toHaveCount(0);
  await expect(app.page.getByRole("listbox")).toHaveCount(0);
  expect(app.popups).toEqual([]);
  expect(app.page.context().pages()).toHaveLength(1);

  await app.press("Escape");
  await expect(app.list).toBeVisible();
  await expect(app.highlighted).toContainText("Person 4");

  await app.row("Person 5").click();
  await expect(app.conversationTitle).toHaveText("Person 5");
  await expect(app.list).toHaveCount(0);
});

test("NAV-04 j and k inside a conversation open the next and previous conversation", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  await app.press("j", "Enter");
  await expect(app.conversationTitle).toHaveText("Person 2");

  await app.press("j");
  await expect(app.conversationTitle).toHaveText("Person 3");
  await expect(app.messageBody(0).getByText("quokka")).toBeVisible();
  await app.press("k");
  await expect(app.conversationTitle).toHaveText("Person 2");
  await app.press("k");
  await expect(app.conversationTitle).toHaveText("Person 1");

  await app.press("Escape");
  await expect(app.highlighted).toContainText("Person 1");
  await app.settle();
  await expect(app.row("Person 3")).not.toHaveAttribute("data-unread");

  await app.row("Latest rule").click();
  await expect(app.conversationTitle).toHaveText("Latest rule");
  await app.press("j");
  await expect(app.conversationTitle).toHaveText("Digest: week 44");
});

test("NAV-05 g jumps and the rail", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  const jumps: [string, string, string | null][] = [
    ["e", "Done", null],
    ["h", "Snoozed", null],
    ["s", "Starred", "Trip ideas"],
    ["t", "Sent", "Sent 10 days ago"],
    ["d", "Drafts", null],
    ["#", "Trash", "Old trash"],
    ["!", "Spam", "Old spam"],
  ];
  await inOrder(jumps, async ([key, name, row]) => {
    await app.press("g", key);
    await expectHeader(app, name, `g ${key}`);
    if (row) await expect(app.row(row)).toHaveCount(1);
  });

  await app.press("g", "l");
  await expect(app.page.getByRole("dialog", { name: "Go to label" })).toBeVisible();
  await app.type("Work/Clients");
  await app.press("Enter");
  await expectHeader(app, "Work/Clients");

  await app.press("g", "s");
  await expectHeader(app, "Starred");
  await app.press("g", "i");
  await expectHeader(app, INBOX_HEADER);
  await expect(app.highlighted).toContainText("Person 1");
  await app.press("g", "s");
  await expectHeader(app, "Starred");
  await app.press("g", "o");
  await expectHeader(app, INBOX_HEADER);
  await expect(app.highlighted).toContainText("Digest: week 44");
  await expect(app.activeSplit).toHaveText("Other");

  const rail = app.page.getByRole("navigation");
  const icons: [string, string][] = [
    ["Done (g e)", "Done"],
    ["Snoozed (g h)", "Snoozed"],
    ["Starred (g s)", "Starred"],
    ["Sent (g t)", "Sent"],
    ["Drafts (g d)", "Drafts"],
    ["Inbox", INBOX_HEADER],
  ];
  await inOrder(icons, async ([title, name]) => {
    await rail.getByRole("button", { name: title, exact: true }).click();
    await expectHeader(app, name, title);
  });
});

test("NAV-06 empty states", async ({ app, gmail }) => {
  await seeds.empty(gmail, app);
  await expect(app.page.getByText("You're all done.")).toBeVisible();
  await expect(app.page.getByText("Inbox zero.")).toBeVisible();
  await expect(app.rows).toHaveCount(0);

  await app.press("g", "s");
  await expectHeader(app, "Starred");
  await expect(app.page.getByText("Nothing here.")).toBeVisible();
  await expect(app.rows).toHaveCount(0);

  await app.page
    .getByRole("navigation")
    .getByRole("button", { name: "Inbox", exact: true })
    .click();
  await gmail.deliver({ account: A, from: "Solo <solo@x.test>", subject: "Only one" });
  await app.settle();
  await expect(app.rows).toHaveCount(1);
  await expect(app.highlighted).toContainText("Only one");
  await app.press("e");
  await expect(app.rows).toHaveCount(0);
  await expect(app.page.getByText("Inbox zero.")).toBeVisible();
});

test("NAV-07 the highlight moves when a poll removes the highlighted row", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  const leaveInbox = async (key: string) => {
    await gmail.modify(A, s.latest(key).id, [], ["INBOX"], { poll: true });
    await app.settle();
    await expect(app.row(s.thread(key).subject)).toHaveCount(0);
  };

  await repeat(app, "j", 2);
  await expect(app.highlighted).toContainText("Person 3");
  await leaveInbox("p3");
  await expect(app.highlighted).toContainText("Person 4");

  await app.press("g", "o");
  await expect(app.highlighted).toContainText("Digest: week 44");
  await repeat(app, "j", OTHER.length - 1);
  await expect(app.highlighted).toContainText("Your receipt");
  await leaveInbox("receipt");
  await expect(app.highlighted).toContainText("Forum thread");

  await app.row("Person 5").click();
  await expect(app.conversationTitle).toHaveText("Person 5");
  await leaveInbox("p5");
  await expect(app.conversationTitle).toHaveText("Person 5");
  await expect(app.messages).toHaveCount(1);
  await expect(app.messages.first()).toContainText("Tom");
  await app.press("j");
  await expect(app.conversationTitle).toHaveText("Person 6");
});

const CONVERSATION_SUBS = `Object.fromEntries(Object.entries(window.__sm.useMirror.getState().subs)
  .filter(([, s]) => s.type === "conversation").map(([id, s]) => [String(s.key.thread), id]))`;

test("NAV-08 j keeps the prefetched neighbors it still needs", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  await expect(app.highlighted).toContainText("Person 1");
  await app.settle();
  const p1 = await app.rowKey("Person 1");
  const p2 = await app.rowKey("Person 2");
  const before = await app.page.evaluate<Record<string, string>>(CONVERSATION_SUBS);
  expect(Object.keys(before).toSorted()).toEqual([p1, p2].toSorted());

  await app.press("j");
  await app.settle();
  const after = await app.page.evaluate<Record<string, string>>(CONVERSATION_SUBS);
  expect(Object.keys(after)).toHaveLength(3);
  expect(Object.keys(after)).toEqual(expect.arrayContaining([p1, p2]));
});

test("NAV-10 the highlight follows the mouse, so e acts on the row under it", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  await expect(app.highlighted).toContainText("Person 1");
  const centre = async (text: string) => {
    const box = await app.row(text).boundingBox();
    if (!box) throw new Error(`no row ${text}`);
    return { x: box.x + box.width / 2, y: box.y + box.height / 2 };
  };

  const p3 = await centre("Person 3");
  await app.page.mouse.move(p3.x, p3.y);
  await expect(app.highlighted).toContainText("Person 3");
  await expect(app.page.locator("[role=option].bg-accent-soft")).toHaveCount(1);

  await app.press("j");
  await expect(app.highlighted).toContainText("Person 4");
  await app.page.mouse.move(p3.x, p3.y);
  await expect(app.highlighted).toContainText("Person 4");

  const p2 = await centre("Person 2");
  await app.page.mouse.move(p2.x, p2.y);
  await expect(app.highlighted).toContainText("Person 2");
  await app.press("e");
  await expect(app.row("Person 2")).toHaveCount(0);
  await app.settle();
  const p2msg = (await gmail.gmail(A)).messages.find((m) => m.id === s.latest("p2").id);
  expect(p2msg?.labels).not.toContain("INBOX");

  await app.page.setViewportSize({ width: 1000, height: 420 });
  const scroller = app.list.locator("xpath=..");
  const cut = await app.rows.evaluateAll((rows) => {
    const list = rows[0]?.closest("[role=listbox]")?.parentElement?.getBoundingClientRect();
    const r = rows.find((row) => {
      const b = row.getBoundingClientRect();
      return list !== undefined && b.top < list.bottom - 4 && b.bottom > list.bottom + 4;
    });
    return r ? { y: r.getBoundingClientRect().top + 4, key: r.getAttribute("data-row") } : null;
  });
  expect(cut, "a row cut off by the bottom edge").not.toBeNull();
  const before = await scroller.evaluate((el) => el.scrollTop);
  await app.page.mouse.move(500, cut?.y ?? 0);
  await expect(app.highlighted).toHaveAttribute("data-row", cut?.key ?? "");
  expect(await scroller.evaluate((el) => el.scrollTop)).toBe(before);
});

async function labelsOf(gmail: Control, id: string): Promise<string[]> {
  return (await gmail.gmail(A)).messages.find((m) => m.id === id)?.labels ?? [];
}

test("NAV-11 a poll that empties the list clears the highlight, so # acts on the mail that arrives next", async ({
  app,
  gmail,
}) => {
  await seeds.empty(gmail, app);
  const old = await gmail.deliver({ account: A, from: "Old <old@x.test>", subject: "Old one" });
  await expect(app.highlighted).toContainText("Old one");

  await gmail.modify(A, old.id, [], ["INBOX"], { poll: true });
  await app.settle();
  await expect(app.page.getByText("You're all done.")).toBeVisible();
  await expect(app.highlighted).toHaveCount(0);

  const fresh = await gmail.deliver({ account: A, from: "New <new@x.test>", subject: "New one" });
  await expect(app.highlighted).toContainText("New one");
  await app.settle();
  await app.press("#");
  await expect(app.row("New one")).toHaveCount(0);
  await app.settle();
  expect(await labelsOf(gmail, fresh.id)).toContain("TRASH");
  const oldLabels = await labelsOf(gmail, old.id);
  expect(oldLabels).not.toContain("TRASH");
  expect(oldLabels).not.toContain("INBOX");
});

test("NAV-12 a row removed while another account is on screen moves that account's highlight", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  await repeat(app, "j", 2);
  await expect(app.highlighted).toContainText("Person 3");
  await app.account(seeds.B).click();
  await expect(app.row("B 1")).toHaveCount(1);
  await app.press("j");
  await expect(app.highlighted).toContainText("B 2");

  await gmail.modify(A, s.latest("p3").id, [], ["INBOX"], { poll: true });
  await app.settle();

  await app.account(A).click();
  await expect(app.row("Person 4")).toHaveCount(1);
  await expect(app.row("Person 3")).toHaveCount(0);
  await expect(app.highlighted).toContainText("Person 4");
  await app.press("#");
  await expect(app.row("Person 4")).toHaveCount(0);
  await app.settle();
  expect(await labelsOf(gmail, s.latest("p4").id)).toContain("TRASH");
  expect(await labelsOf(gmail, s.latest("p3").id)).not.toContain("TRASH");
});

test("NAV-13 g #, g !, and a held g jump as typed on a keyboard, and touch no mail", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  await expect(app.highlighted).toContainText("Person 1");
  await app.press("x");
  await expect(app.selected).toHaveCount(1);

  await app.press("g", "#");
  await expectHeader(app, "Trash");
  await app.press("g", "i");
  await expectHeader(app, INBOX_HEADER);
  await app.press("g", "!");
  await expectHeader(app, "Spam");
  await app.press("g", "i");
  await expectHeader(app, INBOX_HEADER);

  const keyboard = app.page.keyboard;
  await keyboard.down("g");
  await keyboard.down("g");
  await keyboard.up("g");
  await app.press("e");
  await expectHeader(app, "Done");

  await app.settle();
  const labels = await labelsOf(gmail, s.latest("p1").id);
  expect(labels).toContain("INBOX");
  expect(labels).not.toContain("TRASH");
  expect(labels).not.toContain("SPAM");
});

test("NAV-14 a list scrolled away from the highlight stays put while mail changes; j keeps the highlight in view", async ({
  app,
  gmail,
}) => {
  const s = await seeds.bulk(gmail, app);
  await expect(app.highlighted).toContainText("Bulk 01");
  const top = await app.rowKey("Bulk 01");
  const highlightStays = () =>
    expect(app.list).toHaveAttribute("aria-activedescendant", `row-${top}`);
  const scroller = app.list.locator("xpath=..");
  const scrollTop = () => scroller.evaluate((el) => el.scrollTop);
  await app.highlighted.hover();
  await app.page.mouse.wheel(0, 10_000);
  await expect
    .poll(() => scroller.evaluate((el) => el.scrollTop + el.clientHeight >= el.scrollHeight - 1))
    .toBe(true);
  await app.settle();
  const end = await scrollTop();
  expect(end).toBeGreaterThan(0);

  await gmail.modify(A, s.latest("important1").id, [], ["UNREAD"], { poll: true });
  await app.settle();
  await highlightStays();
  expect(await scrollTop()).toBe(end);
  await gmail.deliver({ account: A, from: "New <new@x.test>", subject: "Fresh mail" });
  await app.settle();
  await highlightStays();
  expect(await scrollTop()).toBe(end);

  await inOrder(
    Array.from({ length: 39 }, (_, i) => i + 2),
    async (n) => {
      await app.press("j");
      await expect(app.highlighted).toContainText(`Bulk ${String(n).padStart(2, "0")}`);
      await expect.poll(() => highlightInView(app), { message: `at Bulk ${n}` }).toBe(true);
    },
  );
  await expectActiveDescendant(app);
});
