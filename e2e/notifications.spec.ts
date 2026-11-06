// Notifications and the Dock badge (SPEC Notifications).
import type { App } from "./app.ts";
import type { Control, Delivery } from "./control.ts";
import { expect, test } from "./fixtures.ts";
import * as seeds from "./seeds.ts";
import { A, B, C, dates } from "./seeds.ts";

/** The Dock badge; 0 when cleared. */
async function badge(gmail: Control): Promise<number> {
  return (await gmail.platform()).badge ?? 0;
}

async function notes(gmail: Control) {
  return (await gmail.platform()).notifications;
}

/** The unread count on an account's avatar; 0 when it shows none. */
async function avatarCount(app: App, email: string): Promise<number> {
  const count = app.account(email).locator("span.tabular");
  return (await count.count()) === 0 ? 0 : Number(await count.innerText());
}

/** A new unread Inbox message in its own thread, not polled yet. */
function fresh(account: string, subject: string, extra: Partial<Delivery> = {}): Delivery {
  return {
    account,
    from: "Nia New <nia@new.test>",
    subject,
    text: `${subject}.`,
    labels: ["INBOX", "UNREAD", "CATEGORY_PERSONAL"],
    poll: false,
    ...extra,
  };
}

/** `n` new unread Inbox messages, tagged `<tag> 1`, `<tag> 2`, … */
function many(account: string, n: number, tag: string): Delivery[] {
  return Array.from({ length: n }, (_, i) => fresh(account, `${tag} ${i + 1}`));
}

/** Runs a palette command by name. */
async function command(app: App, name: string): Promise<void> {
  await app.press("Meta+k");
  await expect(app.palette).toBeVisible();
  await app.type(name);
  await expect(app.palette.locator("[cmdk-item]").first()).toContainText(name);
  await app.press("Enter");
  await expect(app.palette).toHaveCount(0);
}

/** The Mute notifications switches in Settings, one per account in rail order. */
function muteSwitch(app: App, nth: number) {
  return app.sheet("Settings").getByRole("switch", { name: "Mute notifications" }).nth(nth);
}

test.describe("notifications", () => {
  test("NOT-01 new mail in either account raises a notification and adds to the Dock badge", async ({
    app,
    gmail,
  }) => {
    await seeds.standard(gmail, app);
    // A's unread Inbox threads plus B's.
    const start = await badge(gmail);
    expect(start).toBe(9 + 2);
    const [a0, b0] = [await avatarCount(app, A), await avatarCount(app, B)];

    await gmail.deliverMany([fresh(A, "News for Ana"), fresh(B, "News for Ben")]);
    await gmail.poll();
    await app.settle();

    const n = await notes(gmail);
    expect(n.map((x) => x.account).toSorted()).toEqual([A, B]);
    for (const x of n) expect(x.body).toContain(x.account);
    expect(await badge(gmail)).toBe(start + 2);
    expect(await avatarCount(app, A)).toBe(a0 + 1);
    expect(await avatarCount(app, B)).toBe(b0 + 1);
  });

  test("NOT-02 when a notification is not raised", async ({ app, gmail }) => {
    const s = await seeds.standard(gmail, app);
    const p2 = s.thread("p2");
    const reply = (text: string) =>
      gmail.deliver({
        account: A,
        from: "Carol Chen <carol@corp.test>",
        subject: "Re: Person 2",
        text,
        threadId: p2.threadId,
        inReplyTo: s.latest("p2").messageId,
      });
    /** Runs one row of the table: its notification count and badge change. */
    const row = async (act: () => Promise<unknown>, notified: number, badgeDelta: number) => {
      await app.settle();
      await gmail.clearRecorded();
      const before = await badge(gmail);
      await act();
      await app.settle();
      expect(await notes(gmail)).toHaveLength(notified);
      expect(await badge(gmail)).toBe(before + badgeDelta);
    };

    // From a connected account's address.
    await row(
      () => gmail.deliver(fresh(A, "From Ben", { from: `Zed Bolt <${B}>`, poll: true })),
      0,
      1,
    );

    // The thread is open in the focused window: no notification; read on arrival.
    await app.row("Person 2").click();
    await expect(app.conversationTitle).toHaveText("Person 2");
    await row(() => reply("Reply while open."), 0, 0);

    // Only highlighted (prefetched) in the focused window.
    await app.press("Escape");
    await expect(app.highlighted).toContainText("Person 2");
    await expect(app.highlighted).not.toHaveAttribute("data-unread");
    await row(() => reply("Reply while highlighted."), 1, 1);

    // Added without UNREAD, or without INBOX.
    await row(
      () => gmail.deliver(fresh(A, "Already read", { labels: ["INBOX"], poll: true })),
      0,
      0,
    );
    await row(
      () => gmail.deliver(fresh(A, "Skipped Inbox", { labels: ["UNREAD"], poll: true })),
      0,
      0,
    );

    // A new account's connect sync.
    await row(
      async () => {
        await gmail.nextSignIn({ email: C });
        await command(app, "Add account");
        await expect(app.account(C)).toBeVisible();
      },
      0,
      3,
    );

    // The thread is open but the window is unfocused: it notifies.
    await app.press("Control+1");
    await app.row("Person 2").click();
    await expect(app.conversationTitle).toHaveText("Person 2");
    await app.settle();
    await app.focus(false);
    await row(() => reply("Reply while away."), 1, 1);
  });

  test("NOT-03 more than three new messages in one poll become one summary", async ({
    app,
    gmail,
  }) => {
    await seeds.standard(gmail, app);
    const poll = async (list: Delivery[]) => {
      await gmail.clearRecorded();
      await gmail.deliverMany(list);
      await gmail.poll();
      await app.settle();
      return notes(gmail);
    };

    const three = await poll(many(A, 3, "Three"));
    expect(three).toHaveLength(3);
    expect(three.every((x) => x.account === A && x.thread !== null)).toBe(true);

    const four = await poll(many(A, 4, "Four"));
    expect(four).toEqual([expect.objectContaining({ account: A, thread: null })]);

    const mixed = await poll([...many(A, 4, "Mixed"), ...many(B, 2, "Mixed")]);
    expect(mixed.filter((x) => x.account === A)).toEqual([
      expect.objectContaining({ account: A, thread: null }),
    ]);
    const forB = mixed.filter((x) => x.account === B);
    expect(forB).toHaveLength(2);
    expect(forB.every((x) => x.thread !== null)).toBe(true);
  });

  test("NOT-04 a muted account does not notify and is left out of the badge", async ({
    app,
    gmail,
  }) => {
    await seeds.standard(gmail, app);
    const items = app.palette.locator("[cmdk-item]");

    // Mute A from the palette; the palette then offers Unmute.
    await command(app, "Mute notifications");
    await app.settle();
    await app.press("Meta+k");
    await expect(items.filter({ hasText: "Unmute notifications" })).toHaveCount(1);
    await app.press("Escape");
    await expect(app.palette).toHaveCount(0);

    const a0 = await avatarCount(app, A);
    await gmail.clearRecorded();
    await gmail.deliver(fresh(A, "While muted", { poll: true }));
    await app.settle();
    expect(await notes(gmail)).toEqual([]);
    expect(await avatarCount(app, A)).toBe(a0 + 1);
    // B's two unread threads only.
    expect(await badge(gmail)).toBe(2);

    // Settings shows A muted.
    await app.press("Meta+,");
    await expect(muteSwitch(app, 0)).toHaveAttribute("aria-checked", "true");
    await expect(muteSwitch(app, 1)).toHaveAttribute("aria-checked", "false");
    await app.press("Escape");
    await expect(app.sheet("Settings")).toHaveCount(0);

    // Unmute: A counts again and notifies again.
    await command(app, "Unmute notifications");
    await app.settle();
    // A's 9 seeded unread threads and "While muted", plus B's 2.
    expect(await badge(gmail)).toBe(12);
    await gmail.clearRecorded();
    await gmail.deliver(fresh(A, "After unmute", { poll: true }));
    await app.settle();
    expect(await notes(gmail)).toEqual([expect.objectContaining({ account: A })]);

    // Mute B from Settings with A on screen; the palette still acts on A.
    await app.press("Meta+,");
    await muteSwitch(app, 1).click();
    await expect(muteSwitch(app, 1)).toHaveAttribute("aria-checked", "true");
    await expect(muteSwitch(app, 0)).toHaveAttribute("aria-checked", "false");
    await app.press("Escape");
    await app.settle();
    // A's 10 and "After unmute"; B left out.
    expect(await badge(gmail)).toBe(11);
    await app.press("Meta+k");
    await expect(items.filter({ hasText: "Mute notifications" })).toHaveCount(1);
    await expect(items.filter({ hasText: "Unmute notifications" })).toHaveCount(0);
    await app.press("Escape");
    await command(app, "Mute notifications");
    await app.settle();
    await app.press("Meta+,");
    await expect(muteSwitch(app, 0)).toHaveAttribute("aria-checked", "true");
    await expect(muteSwitch(app, 1)).toHaveAttribute("aria-checked", "true");
    expect((await gmail.platform()).badge).toBeNull();
  });

  test("NOT-05 the Dock badge counts unread Inbox threads", async ({ app, gmail }) => {
    const s = await seeds.standard(gmail, app);
    // `plan` has three unread messages and counts once: A's 9 unread threads plus B's 2.
    expect(await badge(gmail)).toBe(11);

    // Open an unread thread: read on open.
    await app.row("Person 1").click();
    await expect(app.conversationTitle).toHaveText("Person 1");
    await app.press("Escape");
    await app.settle();
    expect(await badge(gmail)).toBe(10);

    // `u` on a read Inbox thread.
    await app.press("j");
    await expect(app.highlighted).toContainText("Person 2");
    await app.press("u");
    await app.settle();
    expect(await badge(gmail)).toBe(11);

    // Archive an unread thread: e marks it read, and it is no longer counted.
    await app.press("j");
    await expect(app.highlighted).toContainText("Person 3");
    await app.press("e");
    await app.settle();
    expect(await badge(gmail)).toBe(10);
    const p3 = (await gmail.gmail(A)).messages.find((m) => m.id === s.message("p3", 1).id);
    expect(p3?.labels).not.toContain("UNREAD");
    expect(p3?.labels).not.toContain("INBOX");

    // Snooze an unread thread.
    await expect(app.highlighted).toContainText("Person 4");
    await app.press("j");
    await expect(app.highlighted).toContainText("Person 5");
    await app.press("h");
    await expect(app.palette).toBeVisible();
    await app.press("Enter");
    await expect(app.row("Person 5")).toHaveCount(0);
    await app.settle();
    expect(await badge(gmail)).toBe(9);

    // Every Inbox thread read, and B muted: the badge is cleared.
    const unread = (await gmail.gmail(A)).messages.filter(
      (m) => m.labels.includes("INBOX") && m.labels.includes("UNREAD"),
    );
    await Promise.all(unread.map((m) => gmail.modify(A, m.id, [], ["UNREAD"])));
    await gmail.poll();
    await app.settle();
    expect(await badge(gmail)).toBe(2);
    await app.press("Meta+,");
    await muteSwitch(app, 1).click();
    await expect(muteSwitch(app, 1)).toHaveAttribute("aria-checked", "true");
    await app.settle();
    expect((await gmail.platform()).badge).toBeNull();
  });

  test("NOT-06 clicking a notification opens that account's Inbox on the thread", async ({
    app,
    gmail,
  }) => {
    await seeds.standard(gmail, app);
    await expect(app.row("Person 1")).toBeVisible();

    // A notification for B, clicked (focus within 30 s): B's Inbox on the thread.
    await app.focus(false);
    // Older than B 1, so it is not the row highlighted by default.
    const u = await gmail.deliver(
      fresh(B, "Urgent for Ben", { poll: true, date: dates.minutesAgo(20) }),
    );
    await app.settle();
    expect(await app.focus(true)).toEqual({ account: B, thread: u.threadId });
    await expect(app.highlighted).toContainText("Urgent for Ben");
    await expect(app.row("B 2")).toBeVisible();
    await expect(app.row("Person 1")).toHaveCount(0);

    // A summary opens B's Inbox, wherever B was.
    await app.press("g", "e");
    await expect(app.row("B 2")).toHaveCount(0);
    await app.press("Control+1");
    await expect(app.row("Person 1")).toBeVisible();
    await app.focus(false);
    await gmail.deliverMany(Array.from({ length: 4 }, (_, i) => fresh(B, `Batch ${i + 1}`)));
    await gmail.poll();
    await app.settle();
    expect(await app.focus(true)).toEqual({ account: B, thread: null });
    await expect(app.row("B 2")).toBeVisible();
    await expect(app.row("Batch 4")).toBeVisible();

    // With a touched composer open on A: it closes first, and the draft is kept.
    await app.press("Control+1");
    await expect(app.row("Person 1")).toBeVisible();
    await app.compose();
    await app.subject.fill("Kept through a notification");
    await app.settle();
    await app.focus(false);
    await gmail.deliver(fresh(B, "Another for Ben", { poll: true }));
    await app.settle();
    expect(await app.focus(true)).toEqual(expect.objectContaining({ account: B }));
    await expect(app.highlighted).toContainText("Another for Ben");
    await expect(app.page.getByRole("heading", { name: "New Message" })).toHaveCount(0);
    await app.settle();
    const drafts = (await gmail.gmail(A)).drafts;
    expect(drafts.map((d) => d.message.headers.subject)).toEqual(["Kept through a notification"]);

    // Focus more than 30 s after the notification is not a click.
    await app.press("Control+1");
    await expect(app.row("Person 1")).toBeVisible();
    await app.focus(false);
    await gmail.deliver(fresh(B, "Late for Ben", { poll: true }));
    await app.advance(31_000);
    expect(await app.focus(true)).toBeNull();
    await app.settle();
    await expect(app.row("Person 1")).toBeVisible();
    await expect(app.row("Late for Ben")).toHaveCount(0);

    // A notification shown while the window had focus is not clicked by a later refocus.
    await gmail.clearRecorded();
    const seen = await gmail.deliver(fresh(B, "Shown in front", { poll: true }));
    await app.settle();
    expect((await notes(gmail)).map((x) => x.thread)).toEqual([seen.threadId]);
    await app.advance(1_000);
    await app.focus(false);
    expect(await app.focus(true)).toBeNull();
    await app.settle();
    await expect(app.row("Person 1")).toBeVisible();
  });
});
