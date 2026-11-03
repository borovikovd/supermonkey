// Snooze (COVERAGE 5.10, SPEC §Snooze): the picker, snoozing, the wake
// triggers, the clearing rules, the history echo, and undo. The clock starts
// on Monday 2026-11-02 10:00 in New York.
import type { App } from "./app.ts";
import type { Control } from "./control.ts";
import { expect, test } from "./fixtures.ts";
import * as seeds from "./seeds.ts";
import { A, B, dates } from "./seeds.ts";

const TUE_8AM = "Nov 3";
const FRI_9AM = "Nov 6";

/** Runs `step` on each item, one after another. */
async function inOrder<T>(items: readonly T[], step: (item: T) => Promise<void>): Promise<void> {
  await items.reduce((prev, item) => prev.then(() => step(item)), Promise.resolve());
}

function picker(app: App) {
  return app.page.getByRole("dialog", { name: /^Snooze until/ });
}

/** Presses `h` and picks a named option ("Later today", …). */
async function snoozeNamed(app: App, name: string): Promise<void> {
  await app.press("h");
  await expect(picker(app)).toBeVisible();
  await picker(app)
    .getByRole("option", { name: new RegExp(`^${name}`) })
    .click();
  await expect(picker(app)).toHaveCount(0);
}

/** Presses `h`, types a date-and-time phrase (Q13), and picks what it offers. */
async function snoozeTyped(app: App, phrase: string, offered: string): Promise<void> {
  await app.press("h");
  await expect(picker(app)).toBeVisible();
  await app.type(phrase);
  await expect(picker(app).getByRole("option").first()).toHaveText(offered);
  await app.press("Enter");
  await expect(picker(app)).toHaveCount(0);
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

type ViewKey = "i" | "h" | "e" | "#" | "!";

const RAIL: Partial<Record<ViewKey, string | RegExp>> = {
  i: "Inbox",
  h: /^Snoozed/,
  e: /^Done/,
};
const PALETTE: Partial<Record<ViewKey, string>> = { "#": "Go to Trash", "!": "Go to Spam" };

/**
 * Opens a view from the rail or the palette. Not with `g` keys: a stale `g`
 * timeout can swallow a later `g` prefix (src/keyboard/keys.ts:252).
 */
async function goView(app: App, view: ViewKey): Promise<void> {
  const rail = RAIL[view];
  if (rail !== undefined) {
    await app.page.getByRole("navigation").getByRole("button", { name: rail, exact: true }).click();
    return;
  }
  await app.press("Meta+k");
  await app.palette.getByRole("option", { name: new RegExp(`^${PALETTE[view] ?? "?"}`) }).click();
  await expect(app.palette).toHaveCount(0);
}

/**
 * A history poll of A that really ran: `poll()` can wake another account's
 * loop instead (known bug: poll_kick wakes only one waiter).
 */
async function pollA(app: App, gmail: Control): Promise<void> {
  const polls = async () =>
    (await gmail.requests("history.list", { account: A })).filter((r) => r.answer === 200).length;
  const before = await polls();
  await expect
    .poll(async () => {
      await gmail.poll();
      await app.settle();
      return polls();
    })
    .toBeGreaterThan(before);
}

/** Opens a view with `g` and a key, and says whether a row with this subject is listed. */
async function listed(app: App, view: ViewKey, subject: string): Promise<boolean> {
  await goView(app, view);
  await app.settle();
  // The view's first load is in: rows, or its empty state.
  await expect(
    app.rows.first().or(app.page.getByText(/^(You're all done\.|Nothing here\.)$/)),
  ).toBeVisible();
  return (await app.row(subject).count()) > 0;
}

async function expectIn(app: App, view: ViewKey, subject: string): Promise<void> {
  await goView(app, view);
  await expect(app.row(subject)).toHaveCount(1);
}

async function expectNotIn(app: App, view: ViewKey, subject: string): Promise<void> {
  expect(await listed(app, view, subject), `${subject} not in g ${view}`).toBe(false);
}

/** Whether any non-draft message of the thread has `INBOX` in Gmail. */
async function gmailInbox(gmail: Control, threadId: string, account = A): Promise<boolean> {
  const state = await gmail.gmail(account);
  return state.messages
    .filter((m) => m.threadId === threadId && !m.labels.includes("DRAFT"))
    .some((m) => m.labels.includes("INBOX"));
}

function wakeTime(app: App, subject: string) {
  return app.row(subject).locator(".tabular");
}

test("SNZ-01 the snooze picker at a normal hour", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  await expect(app.highlighted).toContainText("Person 1");
  await app.press("h");
  await expect(picker(app)).toBeVisible();
  // Today is Monday, so next week is the Monday after.
  await expect(picker(app).getByRole("option")).toHaveText([
    /^Later today\b.*Mon Nov 2, 6:00 PM$/,
    /^Tomorrow morning\b.*Tue Nov 3, 8:00 AM$/,
    /^Next week\b.*Mon Nov 9, 8:00 AM$/,
  ]);
  await app.type("fri 9am");
  await expect(picker(app).getByRole("option").first()).toHaveText("Fri Nov 6, 9:00 AM");
  await app.press("Escape");
  await expect(picker(app)).toHaveCount(0);
  await app.settle();
  // Only the picker closed: still on the Inbox list, the row untouched.
  await expect(app.list).toBeVisible();
  await expect(app.row("Person 1")).toHaveCount(1);
  await expect(app.highlighted).toContainText("Person 1");
  expect(await gmail.requests("messages.batchModify")).toEqual([]);
});

test("SNZ-02 Later today disappears at 18:00", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  await app.advanceTo(dates.at("2026-11-02T17:59"));
  await app.press("h");
  await expect(picker(app).getByRole("option", { name: /^Later today/ })).toHaveCount(1);
  await app.press("Escape");
  await expect(picker(app)).toHaveCount(0);
  await app.advanceTo(dates.at("2026-11-02T18:00"));
  await app.press("h");
  await expect(picker(app).getByRole("option", { name: /^Tomorrow morning/ })).toHaveCount(1);
  await expect(picker(app).getByRole("option", { name: /^Later today/ })).toHaveCount(0);
});

test("SNZ-03 snoozing takes the thread out of Inbox, here and in Gmail", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  const labelsBefore = (await gmail.gmail(A)).labels;
  const hold = await gmail.hold("messages.batchModify");
  await app.press("x", "Shift+J");
  await expect(app.selected).toHaveCount(2);
  await snoozeNamed(app, "Tomorrow morning");
  // Both rows leave before Gmail answers.
  await expect(app.row("Person 1")).toHaveCount(0);
  await expect(app.row("Person 2")).toHaveCount(0);
  await hold.release();
  await app.settle();

  await goView(app, "h");
  await expect(app.row("Person 1")).toHaveCount(1);
  await expect(app.row("Person 2")).toHaveCount(1);
  await expect(wakeTime(app, "Person 1")).toHaveText(TUE_8AM);
  await expect(wakeTime(app, "Person 2")).toHaveText(TUE_8AM);
  await expectNotIn(app, "e", "Person 1");
  await expectNotIn(app, "e", "Person 2");

  const state = await gmail.gmail(A);
  for (const key of ["p1", "p2"]) {
    const msgs = state.messages.filter((m) => m.threadId === s.thread(key).threadId);
    expect(msgs.length).toBeGreaterThan(0);
    for (const m of msgs) {
      // No INBOX, and no label marks the snooze.
      expect(m.labels).not.toContain("INBOX");
      expect(m.labels.every((l) => l === "CATEGORY_PERSONAL" || l === "UNREAD")).toBe(true);
    }
  }
  expect(state.labels).toEqual(labelsBefore);

  // One z brings both back.
  await goView(app, "i");
  await app.press("z");
  await expect(app.row("Person 1")).toHaveCount(1);
  await expect(app.row("Person 2")).toHaveCount(1);
  await app.settle();
  expect(await gmailInbox(gmail, s.thread("p1").threadId)).toBe(true);
  expect(await gmailInbox(gmail, s.thread("p2").threadId)).toBe(true);
});

test("SNZ-04 snoozed threads wake on time, on launch, and after sleep, even offline", async ({
  app,
  gmail,
}) => {
  test.slow();
  const s = await seeds.standard(gmail, app);
  const p1 = s.thread("p1").threadId;
  const p3 = s.thread("p3").threadId;
  const p4 = s.thread("p4").threadId;

  // Mac wake: p2 until 10:05, then the Mac sleeps 10 minutes; no timer fires.
  await highlightRow(app, "Person 2");
  await snoozeTyped(app, "10:05", "Mon Nov 2, 10:05 AM");
  await expect(app.row("Person 2")).toHaveCount(0);
  await app.sleep(10 * 60_000);
  await app.settle();
  expect(await listed(app, "i", "Person 2"), "not woken while asleep").toBe(false);
  await app.sleepAndWake(0);
  await expectIn(app, "i", "Person 2");
  await expectNotIn(app, "h", "Person 2");

  // Timer: p1 until Later today, then the clock reaches 18:00.
  await goView(app, "i");
  await highlightRow(app, "Person 1");
  await snoozeNamed(app, "Later today");
  await app.settle();
  expect(await gmailInbox(gmail, p1)).toBe(false);
  await app.advanceTo(dates.at("2026-11-02T17:59"));
  expect(await listed(app, "i", "Person 1"), "not woken before 18:00").toBe(false);
  await app.advanceTo(dates.at("2026-11-02T18:00"));
  await expectIn(app, "i", "Person 1");
  await expectNotIn(app, "h", "Person 1");
  await app.settle();
  expect(await gmailInbox(gmail, p1)).toBe(true);

  // Offline at wake: p3 until 19:00; the local Inbox has it at once, Gmail later.
  await goView(app, "i");
  await highlightRow(app, "Person 3");
  await snoozeTyped(app, "7pm", "Mon Nov 2, 7:00 PM");
  await app.settle();
  await gmail.offline(true);
  await app.advanceTo(dates.at("2026-11-02T19:01"));
  await expectIn(app, "i", "Person 3");
  await expectNotIn(app, "h", "Person 3");
  expect(await gmailInbox(gmail, p3)).toBe(false);
  await gmail.offline(false);
  await app.settle();
  await expect.poll(() => gmailInbox(gmail, p3)).toBe(true);

  // Launch: p4 until 20:00; the app is down past it.
  await goView(app, "i");
  await highlightRow(app, "Person 4");
  const p4Key = await app.rowKey("Person 4");
  await snoozeTyped(app, "8pm", "Mon Nov 2, 8:00 PM");
  await app.settle();
  // Record the rows of the first painted list after the relaunch.
  await app.page.addInitScript(() => {
    if (!location.href.startsWith("http")) return;
    const w = window as Window & { e2eFirstRows?: string[] };
    new MutationObserver((_, obs) => {
      const rows = Array.from(document.querySelectorAll("[data-row]"));
      if (rows.length === 0) return;
      w.e2eFirstRows = rows.map((r) => r.getAttribute("data-row") ?? "");
      obs.disconnect();
    }).observe(document, { childList: true, subtree: true });
  });
  await app.restart(async () => {
    await app.advanceTo(dates.at("2026-11-02T21:00"));
    expect(await gmailInbox(gmail, p4), "no INBOX while the app is down").toBe(false);
  });
  const firstRows = await app.page.evaluate(
    () => (window as Window & { e2eFirstRows?: string[] }).e2eFirstRows ?? [],
  );
  expect(firstRows, "the first painted Inbox has p4").toContain(p4Key);
  // Gmail gets INBOX once the queued wake runs.
  await expect.poll(() => gmailInbox(gmail, p4)).toBe(true);
  await expect(app.row("Person 4")).toHaveCount(1);
  await expectNotIn(app, "h", "Person 4");
  await app.settle();
  await expectIn(app, "i", "Person 4");
  expect(await gmailInbox(gmail, p4)).toBe(true);
});

test("SNZ-05 INBOX added from outside clears a snooze; your own reply does not", async ({
  app,
  gmail,
}) => {
  test.slow();
  const s = await seeds.standard(gmail, app);
  const p3 = s.thread("p3");

  const snoozeP3 = async () => {
    await goView(app, "i");
    await highlightRow(app, "Person 3");
    await snoozeNamed(app, "Next week");
    await expectIn(app, "h", "Person 3");
  };
  const expectCleared = async (why: string) => {
    await app.settle();
    await expectIn(app, "i", "Person 3");
    expect(await listed(app, "h", "Person 3"), `${why}: not snoozed`).toBe(false);
  };
  const expectKept = async (why: string) => {
    await app.settle();
    await expectIn(app, "h", "Person 3");
    expect(await listed(app, "i", "Person 3"), `${why}: not in Inbox`).toBe(false);
  };
  const reply = (from: string) => ({
    account: A,
    from,
    to: [A],
    subject: "Re: Person 3",
    text: `A reply from ${from}.`,
    threadId: p3.threadId,
    inReplyTo: s.latest("p3").messageId,
    labels: ["INBOX", "UNREAD", "CATEGORY_PERSONAL"],
    poll: false,
  });

  // A new message from Carol arrives with INBOX.
  await snoozeP3();
  await gmail.deliver(reply("Carol Chen <carol@corp.test>"));
  await pollA(app, gmail);
  await expectCleared("Carol's message");

  // Mail from A's other account.
  await snoozeP3();
  await gmail.deliver(reply(`Zed Bolt <${B}>`));
  await pollA(app, gmail);
  await expectCleared("mail from ben@bolt.test");

  // Another client moves it to the inbox.
  await snoozeP3();
  await gmail.modify(A, s.message("p3", 1).id, ["INBOX"], []);
  await pollA(app, gmail);
  await expectCleared("moved to inbox elsewhere");

  // Another client stars it.
  await snoozeP3();
  await gmail.modify(A, s.message("p3", 1).id, ["STARRED"], []);
  await pollA(app, gmail);
  await expectKept("starred elsewhere");

  // A reply sent from this app, opened from Snoozed.
  await goView(app, "h");
  await highlightRow(app, "Person 3");
  await app.press("Enter");
  await app.reply("r");
  await app.type("My own reply");
  await app.settle();
  await app.press("Meta+Enter");
  await app.advance(10_000);
  await app.settle();
  await pollA(app, gmail);
  await app.press("Escape");
  await expectKept("own reply");
  const sent = (await gmail.gmail(A)).messages.filter(
    (m) => m.threadId === p3.threadId && m.labels.includes("SENT"),
  );
  expect(sent, "the reply was sent").toHaveLength(1);

  // e, #, and ! from Snoozed clear it: Done, Trash, Spam.
  const clearing = [
    ["e", "e", "Person 3"],
    ["#", "#", "Person 5"],
    ["!", "!", "Person 6"],
  ] as const;
  await inOrder(clearing, async ([key, view, subject]) => {
    if (subject !== "Person 3") {
      await goView(app, "i");
      await highlightRow(app, subject);
      await snoozeNamed(app, "Next week");
    }
    await goView(app, "h");
    await highlightRow(app, subject);
    await app.press(key);
    await expect(app.row(subject)).toHaveCount(0);
    await app.settle();
    await expectIn(app, view, subject);
    await expectNotIn(app, "h", subject);
    await expectNotIn(app, "i", subject);
  });
});

test("SNZ-06 the app's own label changes, echoed by history, do not clear a newer snooze", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  const p4 = s.thread("p4").threadId;
  await highlightRow(app, "Person 4");
  await snoozeNamed(app, "Tomorrow morning");
  await app.settle();
  await app.press("z");
  await expect(app.row("Person 4")).toHaveCount(1);
  await app.settle();
  expect(await gmailInbox(gmail, p4), "the undo reached Gmail").toBe(true);

  // Before any poll, snooze it again.
  await highlightRow(app, "Person 4");
  await snoozeTyped(app, "fri 9am", "Fri Nov 6, 9:00 AM");
  await app.settle();
  expect(await gmailInbox(gmail, p4)).toBe(false);

  // One poll returns both of the app's changes: +INBOX, then −INBOX.
  await pollA(app, gmail);
  await app.settle();
  expect(await listed(app, "i", "Person 4"), "not in Inbox").toBe(false);
  await goView(app, "h");
  await expect(app.row("Person 4")).toHaveCount(1);
  await expect(wakeTime(app, "Person 4")).toHaveText(FRI_9AM);
  expect(await gmailInbox(gmail, p4)).toBe(false);
});

test("SNZ-07 undo, Shift+E, and e on snoozed threads", async ({ app, gmail }) => {
  test.slow();
  const s = await seeds.standard(gmail, app);
  const p1 = s.thread("p1").threadId;
  const fromInbox = async () => {
    await goView(app, "i");
    await highlightRow(app, "Person 1");
  };
  const fromSnoozed = async () => {
    await goView(app, "h");
    await highlightRow(app, "Person 1");
  };

  // h, then z: back in Inbox with no snooze; Gmail has INBOX.
  await snoozeNamed(app, "Tomorrow morning");
  await app.settle();
  await app.press("z");
  await expect(app.row("Person 1")).toHaveCount(1);
  await expectNotIn(app, "h", "Person 1");
  await app.settle();
  expect(await gmailInbox(gmail, p1)).toBe(true);

  // h (Tue 08:00), h again from Snoozed (Fri 09:00), then z: Tue 08:00 again.
  await fromInbox();
  await snoozeNamed(app, "Tomorrow morning");
  await fromSnoozed();
  await snoozeTyped(app, "fri 9am", "Fri Nov 6, 9:00 AM");
  await app.settle();
  await goView(app, "h");
  await expect(wakeTime(app, "Person 1")).toHaveText(FRI_9AM);
  await app.press("z");
  await expect(wakeTime(app, "Person 1")).toHaveText(TUE_8AM);
  await expectNotIn(app, "i", "Person 1");
  // Back to a plain Inbox thread: undo the first snooze too.
  await app.press("z");
  await expectIn(app, "i", "Person 1");
  await app.settle();
  expect(await gmailInbox(gmail, p1)).toBe(true);

  // h, then Shift+E from Snoozed: cleared, in Inbox, Gmail INBOX.
  await fromInbox();
  await snoozeNamed(app, "Tomorrow morning");
  await app.settle();
  expect(await gmailInbox(gmail, p1)).toBe(false);
  await fromSnoozed();
  await app.press("Shift+E");
  await expect(app.row("Person 1")).toHaveCount(0);
  await expectIn(app, "i", "Person 1");
  await expectNotIn(app, "h", "Person 1");
  await app.settle();
  expect(await gmailInbox(gmail, p1)).toBe(true);

  // h, then e from Snoozed, then z: Done, then Snoozed until Tue 08:00 again.
  await fromInbox();
  await snoozeNamed(app, "Tomorrow morning");
  await fromSnoozed();
  await app.press("e");
  await expect(app.row("Person 1")).toHaveCount(0);
  await expectIn(app, "e", "Person 1");
  await expectNotIn(app, "h", "Person 1");
  await expectNotIn(app, "i", "Person 1");
  await app.press("z");
  await goView(app, "h");
  await expect(app.row("Person 1")).toHaveCount(1);
  await expect(wakeTime(app, "Person 1")).toHaveText(TUE_8AM);
  await expectNotIn(app, "i", "Person 1");
  await app.settle();
  expect(await gmailInbox(gmail, p1)).toBe(false);
});

test("SNZ-08 Enter before the typed time's options arrive snoozes until that time", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  await expect(app.highlighted).toContainText("Person 1");
  await app.press("h");
  await expect(picker(app).getByRole("option").first()).toContainText("Later today");
  // The options for anything typed wait, so the screen still shows the named ones.
  const release = await app.park("snooze_options", (body) => body.includes("fri"));
  await app.type("fri 9am");
  await app.press("Enter");
  await release();
  await expect(picker(app)).toHaveCount(0);
  await app.settle();
  await goView(app, "h");
  await expect(app.row("Person 1")).toHaveCount(1);
  await expect(wakeTime(app, "Person 1")).toHaveText(FRI_9AM);
});
