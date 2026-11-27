import type { App } from "./app.ts";
import type { Control, Delivery } from "./control.ts";
import { expect, test } from "./fixtures.ts";
import * as seeds from "./seeds.ts";
import { A, B, C, dates } from "./seeds.ts";

async function badge(gmail: Control): Promise<number> {
  return (await gmail.platform()).badge ?? 0;
}

async function notes(gmail: Control) {
  return (await gmail.platform()).notifications;
}

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

function many(account: string, n: number, tag: string): Delivery[] {
  return Array.from({ length: n }, (_, i) => fresh(account, `${tag} ${i + 1}`));
}

function muteSwitch(app: App, nth: number) {
  return app.sheet("Settings").getByRole("switch", { name: "Mute notifications" }).nth(nth);
}

test.describe("notifications", () => {
  test("NOT-01 new mail in either account raises a notification and adds to the Dock badge", async ({
    app,
    gmail,
  }) => {
    await seeds.standard(gmail, app);
    const start = await badge(gmail);
    expect(start).toBe(9 + 2);
    const [a0, b0] = [await app.avatarCount(A), await app.avatarCount(B)];

    await gmail.deliverMany([fresh(A, "News for Ana"), fresh(B, "News for Ben")]);
    await gmail.poll();
    await app.settle();

    const n = await notes(gmail);
    expect(n.map((x) => x.account).toSorted()).toEqual([A, B]);
    for (const x of n) expect(x.body).toContain(x.account);
    expect(await badge(gmail)).toBe(start + 2);
    expect(await app.avatarCount(A)).toBe(a0 + 1);
    expect(await app.avatarCount(B)).toBe(b0 + 1);
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
    const row = async (act: () => Promise<unknown>, notified: number, badgeDelta: number) => {
      await app.settle();
      await gmail.clearRecorded();
      const before = await badge(gmail);
      await act();
      await app.settle();
      expect(await notes(gmail)).toHaveLength(notified);
      expect(await badge(gmail)).toBe(before + badgeDelta);
    };

    await row(
      () => gmail.deliver(fresh(A, "From Ben", { from: `Zed Bolt <${B}>`, poll: true })),
      0,
      1,
    );

    await app.row("Person 2").click();
    await expect(app.conversationTitle).toHaveText("Person 2");
    await row(() => reply("Reply while open."), 0, 0);

    await app.press("Escape");
    await expect(app.highlighted).toContainText("Person 2");
    await expect(app.highlighted).not.toHaveAttribute("data-unread");
    await row(() => reply("Reply while highlighted."), 1, 1);

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

    await row(
      async () => {
        await gmail.nextSignIn({ email: C });
        await app.command("Add account");
        await expect(app.palette).toHaveCount(0);
        await expect(app.account(C)).toBeVisible();
      },
      0,
      3,
    );

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

    await app.command("Mute notifications");
    await expect(app.palette).toHaveCount(0);
    await app.settle();
    await app.press("Meta+k");
    await expect(items.filter({ hasText: "Unmute notifications" })).toHaveCount(1);
    await app.press("Escape");
    await expect(app.palette).toHaveCount(0);

    const a0 = await app.avatarCount(A);
    await gmail.clearRecorded();
    await gmail.deliver(fresh(A, "While muted", { poll: true }));
    await app.settle();
    expect(await notes(gmail)).toEqual([]);
    expect(await app.avatarCount(A)).toBe(a0 + 1);
    expect(await badge(gmail)).toBe(2);

    await app.press("Meta+,");
    await expect(muteSwitch(app, 0)).toHaveAttribute("aria-checked", "true");
    await expect(muteSwitch(app, 1)).toHaveAttribute("aria-checked", "false");
    await app.press("Escape");
    await expect(app.sheet("Settings")).toHaveCount(0);

    await app.command("Unmute notifications");
    await expect(app.palette).toHaveCount(0);
    await app.settle();
    expect(await badge(gmail)).toBe(12);
    await gmail.clearRecorded();
    await gmail.deliver(fresh(A, "After unmute", { poll: true }));
    await app.settle();
    expect(await notes(gmail)).toEqual([expect.objectContaining({ account: A })]);

    await app.press("Meta+,");
    await muteSwitch(app, 1).click();
    await expect(muteSwitch(app, 1)).toHaveAttribute("aria-checked", "true");
    await expect(muteSwitch(app, 0)).toHaveAttribute("aria-checked", "false");
    await app.press("Escape");
    await app.settle();
    expect(await badge(gmail)).toBe(11);
    await app.press("Meta+k");
    await expect(items.filter({ hasText: "Mute notifications" })).toHaveCount(1);
    await expect(items.filter({ hasText: "Unmute notifications" })).toHaveCount(0);
    await app.press("Escape");
    await app.command("Mute notifications");
    await expect(app.palette).toHaveCount(0);
    await app.settle();
    await app.press("Meta+,");
    await expect(muteSwitch(app, 0)).toHaveAttribute("aria-checked", "true");
    await expect(muteSwitch(app, 1)).toHaveAttribute("aria-checked", "true");
    expect((await gmail.platform()).badge).toBeNull();
  });

  test("NOT-05 the Dock badge counts unread Inbox threads", async ({ app, gmail }) => {
    const s = await seeds.standard(gmail, app);
    expect(await badge(gmail)).toBe(11);

    await app.row("Person 1").click();
    await expect(app.conversationTitle).toHaveText("Person 1");
    await app.press("Escape");
    await app.settle();
    expect(await badge(gmail)).toBe(10);

    await app.press("j");
    await expect(app.highlighted).toContainText("Person 2");
    await app.press("u");
    await app.settle();
    expect(await badge(gmail)).toBe(11);

    await app.press("j");
    await expect(app.highlighted).toContainText("Person 3");
    await app.press("e");
    await app.settle();
    expect(await badge(gmail)).toBe(10);
    const p3 = (await gmail.gmail(A)).messages.find((m) => m.id === s.message("p3", 1).id);
    expect(p3?.labels).not.toContain("UNREAD");
    expect(p3?.labels).not.toContain("INBOX");

    await expect(app.highlighted).toContainText("Person 4");
    await app.press("j");
    await expect(app.highlighted).toContainText("Person 5");
    await app.press("h");
    await expect(app.palette).toBeVisible();
    await app.press("Enter");
    await expect(app.row("Person 5")).toHaveCount(0);
    await app.settle();
    expect(await badge(gmail)).toBe(9);

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

    await app.focus(false);
    const u = await gmail.deliver(
      fresh(B, "Urgent for Ben", { poll: true, date: dates.minutesAgo(20) }),
    );
    await app.settle();
    expect(await app.focus(true)).toEqual({ account: B, thread: u.threadId });
    await expect(app.highlighted).toContainText("Urgent for Ben");
    await expect(app.row("B 2")).toBeVisible();
    await expect(app.row("Person 1")).toHaveCount(0);

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

    await app.press("Control+1");
    await expect(app.row("Person 1")).toBeVisible();
    await app.focus(false);
    await gmail.deliver(fresh(B, "Late for Ben", { poll: true }));
    await app.advance(31_000);
    expect(await app.focus(true)).toBeNull();
    await app.settle();
    await expect(app.row("Person 1")).toBeVisible();
    await expect(app.row("Late for Ben")).toHaveCount(0);

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

  test("NOT-07 new mail read by a poll that then fails still notifies once the poll goes through", async ({
    app,
    gmail,
  }) => {
    await seeds.standard(gmail, app);
    await gmail.clearRecorded();
    await gmail.fault("drafts.list", { kind: "status", status: 503 }, { account: A, times: 1 });
    await gmail.deliver(fresh(A, "Read by a failed poll"));
    await gmail.poll();
    await app.settle();
    await expect(app.row("Read by a failed poll")).toHaveCount(1);

    await app.advance(60_000);
    await app.settle();
    const raised = await notes(gmail);
    expect(raised.map((n) => n.account)).toEqual([A]);
    expect(raised[0]?.title).toContain("Nia New");
  });

  test("NOT-08 mail archived while a failed poll waits to retry does not alert later", async ({
    app,
    gmail,
  }) => {
    await seeds.standard(gmail, app);
    await gmail.clearRecorded();
    await gmail.fault("drafts.list", { kind: "status", status: 503 }, { account: A, times: 1 });
    await gmail.deliver(fresh(A, "Archived before retry"));
    await gmail.poll();
    await app.settle();
    await expect(app.row("Archived before retry")).toHaveCount(1);

    await app.row("Archived before retry").click();
    await app.press("e");
    await expect(app.row("Archived before retry")).toHaveCount(0);
    await app.settle();
    await app.advance(60_000);
    await app.settle();
    expect(await notes(gmail)).toEqual([]);
  });
});
