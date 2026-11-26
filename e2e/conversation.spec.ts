import type { App } from "./app.ts";
import { expect, test } from "./fixtures.ts";
import * as seeds from "./seeds.ts";
import { A } from "./seeds.ts";

type Block = {
  id: string;
  kind: "collapsed" | "expanded" | "fold";
  text: string;
  selected: boolean;
};

function readBlocks(app: App): Promise<Block[]> {
  return app.conversation.evaluate((main) => {
    const out: Block[] = [];
    for (const el of Array.from(main.querySelectorAll("[data-message], button"))) {
      const id = el.getAttribute("data-message");
      if (id !== null) {
        const expanded = el.tagName !== "BUTTON";
        const body = el.querySelector("iframe")?.contentDocument?.body?.textContent ?? "";
        out.push({
          id,
          kind: expanded ? "expanded" : "collapsed",
          text: expanded ? body : (el.textContent ?? ""),
          selected: el.getAttribute("aria-current") === "true",
        });
      } else if (/^\s*\d+\s*older messages\s*$/.test(el.textContent ?? "")) {
        out.push({ id: "", kind: "fold", text: el.textContent ?? "", selected: false });
      }
    }
    return out;
  });
}

const numbered = (text: string) => /message (\d+)\./.exec(text)?.[1] ?? "?";

async function layout(app: App, name: (text: string) => string = numbered): Promise<string[]> {
  return (await readBlocks(app)).map((b) =>
    b.kind === "fold"
      ? `fold${/\d+/.exec(b.text)?.[0] ?? "?"}`
      : `${b.kind === "expanded" ? "E" : "c"}${name(b.text)}${b.selected ? "*" : ""}`,
  );
}

async function messageId(
  app: App,
  tag: string,
  name: (text: string) => string = numbered,
): Promise<string> {
  const b = (await readBlocks(app)).find((x) => x.kind !== "fold" && name(x.text) === tag);
  if (!b) throw new Error(`no message ${tag}`);
  return b.id;
}

function scrollOffset(app: App, id: string): Promise<number> {
  return app.page.evaluate((mid) => {
    const el = document.querySelector(`[data-message="${mid}"]`);
    if (!el) return Number.POSITIVE_INFINITY;
    let sc = el.parentElement;
    while (sc && !/(auto|scroll)/.test(getComputedStyle(sc).overflowY)) sc = sc.parentElement;
    if (!sc) return Number.POSITIVE_INFINITY;
    const top = el.getBoundingClientRect().top - sc.getBoundingClientRect().top + sc.scrollTop;
    return Math.abs(sc.scrollTop - top);
  }, id);
}

function inView(app: App, id: string): Promise<boolean> {
  return app.page.evaluate((mid) => {
    const el = document.querySelector(`[data-message="${mid}"]`);
    if (!el) return false;
    let sc = el.parentElement;
    while (sc && !/(auto|scroll)/.test(getComputedStyle(sc).overflowY)) sc = sc.parentElement;
    if (!sc) return false;
    const r = el.getBoundingClientRect();
    const v = sc.getBoundingClientRect();
    return r.top >= v.top - 1 && r.bottom <= v.bottom + 1;
  }, id);
}

const ownNames = (t: string) =>
  t.includes("Here is the budget draft") ? "kim" : t.includes("Thanks, looks fine") ? "ana" : "?";

const p2Names = (t: string) =>
  t.includes("Hello from Quin") ? "q" : (/Reply (\w+)\./.exec(t)?.[1] ?? "?");

const latestRuleNames = (t: string) =>
  t.includes("The first word") ? "m1" : t.includes("A trashed reply") ? "m2" : "?";

const PLAN_DEFAULT = ["c1", "fold7", "c9", "E10", "E11", "E12*"];

const SHORT = { width: 1280, height: 200 };

test.describe("conversation", () => {
  test("CONV-01 a long thread opens with read messages collapsed and folded, scrolled to the first unread", async ({
    app,
    gmail,
  }) => {
    await app.page.setViewportSize(SHORT);
    const s = await seeds.standard(gmail, app);
    const avatar = await app.avatarCount(A);
    await app.row("Q4 plan").click();

    await expect.poll(() => layout(app)).toEqual(PLAN_DEFAULT);
    const m1 = app.messages.first();
    await expect(m1).toHaveAttribute("aria-expanded", "false");
    await expect(m1).toContainText("Carol Chen");
    await expect(m1).toContainText("Plan message 1.");
    await expect(m1).toContainText("Oct 31");
    await expect(app.conversation.locator("iframe[data-frame]")).toHaveCount(3);
    await expect(app.conversation.locator("button[data-message] iframe")).toHaveCount(0);

    const m10 = await messageId(app, "10");
    await expect.poll(() => scrollOffset(app, m10)).toBeLessThanOrEqual(2);

    await app.settle();
    const plan = new Set(s.thread("plan").messages.map((m) => m.id));
    const gm = (await gmail.gmail(A)).messages.filter((m) => plan.has(m.id));
    expect(gm.filter((m) => m.labels.includes("UNREAD"))).toEqual([]);
    expect(await app.avatarCount(A)).toBe(avatar - 1);
    await app.press("Escape");
    await expect(app.row("Q4 plan")).not.toHaveAttribute("data-unread");
  });

  test("CONV-02 fold boundary: four collapsed messages show, five fold", async ({ app, gmail }) => {
    await app.page.setViewportSize(SHORT);
    await seeds.standard(gmail, app);
    await app.row("Fold check").click();
    await expect
      .poll(() => layout(app))
      .toEqual(["c1", "c2", "c3", "c4", "E5", "c6", "fold3", "c10", "E11*"]);
    await expect
      .poll(async () => scrollOffset(app, await messageId(app, "5")))
      .toBeLessThanOrEqual(2);

    await app.press("Escape");
    await app.row("Budget draft").click();
    await expect.poll(() => layout(app, ownNames)).toEqual(["ckim", "Eana*"]);
    await expect
      .poll(async () => scrollOffset(app, await messageId(app, "ana", ownNames)))
      .toBeLessThanOrEqual(2);
  });

  test("CONV-03 expansion is fixed when the conversation opens", async ({ app, gmail }) => {
    await app.page.setViewportSize(SHORT);
    await seeds.standard(gmail, app);
    await app.row("Q4 plan").click();
    await expect.poll(() => layout(app)).toEqual(PLAN_DEFAULT);
    await app.settle();
    await expect.poll(() => layout(app)).toEqual(PLAN_DEFAULT);

    await app.press("p");
    await expect.poll(() => layout(app)).toEqual(["c1", "fold7", "c9", "E10", "E11*", "E12"]);
    await app.press("Escape");
    await expect(app.highlighted).toContainText("Q4 plan");
    await app.press("Enter");
    await expect.poll(() => layout(app)).toEqual(["c1", "fold9", "c11", "E12*"]);
    await expect
      .poll(async () => scrollOffset(app, await messageId(app, "12")))
      .toBeLessThanOrEqual(2);
  });

  test("CONV-04 expanding, collapsing, and selecting with the mouse", async ({ app, gmail }) => {
    await seeds.standard(gmail, app);
    await app.row("Q4 plan").click();
    await expect.poll(() => layout(app)).toEqual(PLAN_DEFAULT);

    await app.messages.first().click();
    await expect.poll(() => layout(app)).toEqual(["E1*", "c2", "fold6", "c9", "E10", "E11", "E12"]);
    await app.messages.first().getByRole("button", { expanded: true }).click();
    await expect.poll(() => layout(app)).toEqual(["c1*", "fold7", "c9", "E10", "E11", "E12"]);

    await app.folds.click();
    await expect(app.folds).toHaveCount(0);
    await expect
      .poll(async () => (await layout(app)).map((t) => t.replace("*", "")))
      .toEqual(["c1", "c2", "c3", "c4", "c5", "c6", "c7", "c8", "c9", "E10", "E11", "E12"]);

    const m11 = await messageId(app, "11");
    await app.conversation
      .locator(`iframe[data-frame="${m11}"]`)
      .contentFrame()
      .locator("body")
      .click();
    await expect(app.selectedMessage).toHaveAttribute("data-message", m11);
  });

  test("CONV-05 n, p, and o select and expand messages", async ({ app, gmail }) => {
    await app.page.setViewportSize(SHORT);
    await seeds.standard(gmail, app);
    await app.row("Q4 plan").click();
    await expect.poll(() => layout(app)).toEqual(PLAN_DEFAULT);
    await app.reply("r");
    await app.type("x");
    await app.settle();
    await app.press("Escape", "Escape");
    await expect(app.editor).toHaveCount(0);
    const stub = app.conversation.getByRole("button", { name: /Draft/ });
    await expect(stub).toHaveCount(1);
    await expect.poll(() => layout(app)).toEqual(PLAN_DEFAULT);

    await app.press("p");
    await expect.poll(() => layout(app)).toEqual(["c1", "fold7", "c9", "E10", "E11*", "E12"]);
    await app.press("p");
    await expect.poll(() => layout(app)).toEqual(["c1", "fold7", "c9", "E10*", "E11", "E12"]);
    await app.press("p");
    await expect.poll(() => layout(app)).toEqual(["c1", "fold7", "c9*", "E10", "E11", "E12"]);
    await app.press("p");
    const opened = ["c1", "c2", "c3", "c4", "c5", "c6", "c7", "c8*", "c9", "E10", "E11", "E12"];
    await expect.poll(() => layout(app)).toEqual(opened);
    const m8 = await messageId(app, "8");
    await expect.poll(() => inView(app, m8)).toBe(true);

    await app.press("o");
    await expect.poll(async () => (await layout(app))[7]).toBe("E8*");
    await app.press("o");
    await expect.poll(async () => (await layout(app))[7]).toBe("c8*");

    await app.press("n", "n", "n", "n");
    await expect.poll(async () => (await layout(app)).at(-1)).toBe("E12*");
    await app.press("n");
    await app.settle();
    expect((await layout(app)).at(-1)).toBe("E12*");
    await expect(app.selectedMessage).toHaveCount(1);
    await expect(stub).not.toHaveAttribute("aria-current");
  });

  test("CONV-06 Shift+O expands everything; pressing it again restores the default", async ({
    app,
    gmail,
  }) => {
    await seeds.standard(gmail, app);
    await app.row("Contract").click();
    const quoted = app.messageBody(0).getByText("The first draft of the contract.");
    await expect(app.messageBody(0).getByText("Please review the contract.")).toBeVisible();
    await expect(quoted).toBeHidden();
    await app.press("Shift+O");
    await expect(quoted).toBeVisible();

    await app.press("Escape");
    await app.row("Q4 plan").click();
    await expect.poll(() => layout(app)).toEqual(PLAN_DEFAULT);
    await app.press("Shift+O");
    await expect(app.conversation.locator("iframe[data-frame]")).toHaveCount(12);
    await expect
      .poll(() => layout(app))
      .toEqual(["E1", "E2", "E3", "E4", "E5", "E6", "E7", "E8", "E9", "E10", "E11", "E12*"]);
    await app.press("Shift+O");
    await expect.poll(() => layout(app)).toEqual(PLAN_DEFAULT);
    await expect(app.conversation.locator("iframe[data-frame]")).toHaveCount(3);
  });

  test("CONV-07 quoted history sits behind •••", async ({ app, gmail }) => {
    await seeds.standard(gmail, app);
    await app.row("Contract").click();
    const body = app.messageBody(0);
    const quoted = body.getByText("The first draft of the contract.");
    await expect(body.getByText("Please review the contract.")).toBeVisible();
    await expect(quoted).toBeHidden();
    await body.getByText("•••").click();
    await expect(quoted).toBeVisible();
    await body.getByText("•••").click();
    await expect(quoted).toBeHidden();
  });

  test("CONV-08 opening marks the thread read locally and on Gmail", async ({ app, gmail }) => {
    const s = await seeds.standard(gmail, app);
    const badge = (await gmail.platform()).badge ?? 0;
    const avatar = await app.avatarCount(A);
    await expect(app.row("Person 1")).toHaveAttribute("data-unread");

    await app.row("Person 1").click();
    await expect(app.conversationTitle).toHaveText("Person 1");
    await app.press("Escape");
    await app.settle();
    await expect(app.row("Person 1")).not.toHaveAttribute("data-unread");
    expect(await app.avatarCount(A)).toBe(avatar - 1);
    expect((await gmail.platform()).badge).toBe(badge - 1);
    expect(await app.gmail.labelsOf(A, s.message("p1", 1).id)).not.toContain("UNREAD");

    await gmail.offline(true);
    await app.row("Person 3").click();
    await expect(app.conversationTitle).toHaveText("Person 3");
    await app.press("Escape");
    await expect(app.row("Person 3")).not.toHaveAttribute("data-unread");
    await app.settle();
    expect(await app.gmail.labelsOf(A, s.message("p3", 1).id)).toContain("UNREAD");
    await gmail.offline(false);
    await app.settle();
    expect(await app.gmail.labelsOf(A, s.message("p3", 1).id)).not.toContain("UNREAD");
  });

  test("CONV-09 a message that arrives while the conversation is open", async ({ app, gmail }) => {
    const s = await seeds.standard(gmail, app);
    const p2 = s.thread("p2");
    const reply = (text: string) =>
      gmail.deliver({
        account: A,
        from: "Carol Chen <carol@corp.test>",
        to: [A],
        subject: "Re: Person 2",
        text,
        threadId: p2.threadId,
        inReplyTo: s.latest("p2").messageId,
      });
    const p2Notes = async () =>
      (await gmail.platform()).notifications.filter((n) => n.thread === p2.threadId);

    await app.row("Person 2").click();
    await expect.poll(() => layout(app, p2Names)).toEqual(["Eq*"]);

    const one = await reply("Reply one.");
    await expect.poll(() => layout(app, p2Names)).toEqual(["Eq", "Eone*"]);
    await app.settle();
    expect(await app.gmail.labelsOf(A, one.id)).not.toContain("UNREAD");
    await app.press("z");
    await app.settle();
    expect(await app.gmail.labelsOf(A, one.id)).not.toContain("UNREAD");
    expect(await layout(app, p2Names)).toEqual(["Eq", "Eone*"]);
    expect(await p2Notes()).toEqual([]);

    await app.press("p");
    await expect.poll(() => layout(app, p2Names)).toEqual(["Eq*", "Eone"]);
    await reply("Reply two.");
    await expect.poll(() => layout(app, p2Names)).toEqual(["Eq*", "Eone", "Etwo"]);

    await app.press("n", "n");
    await expect.poll(() => layout(app, p2Names)).toEqual(["Eq", "Eone", "Etwo*"]);
    await app.focus(false);
    const three = await reply("Reply three.");
    await expect.poll(() => layout(app, p2Names)).toEqual(["Eq", "Eone", "Etwo", "Ethree*"]);
    await app.settle();
    expect(await app.gmail.labelsOf(A, three.id)).toContain("UNREAD");
    expect(await p2Notes()).toHaveLength(1);
  });

  test("CONV-10 if the selected message leaves, the latest is selected", async ({ app, gmail }) => {
    const s = await seeds.standard(gmail, app);
    await app.row("Q4 plan").click();
    await expect.poll(() => layout(app)).toEqual(PLAN_DEFAULT);
    await app.press("p");
    await expect.poll(() => layout(app)).toEqual(["c1", "fold7", "c9", "E10", "E11*", "E12"]);
    await gmail.modify(A, s.message("plan", 11).id, ["TRASH"], [], { poll: true });
    await expect.poll(() => layout(app)).toEqual(["c1", "fold7", "c9", "E10", "E12*"]);
  });

  test("CONV-11 messages in trash or spam are hidden outside Trash and Spam", async ({
    app,
    gmail,
  }) => {
    const s = await seeds.standard(gmail, app);

    await app.row("Latest rule").click();
    await expect.poll(() => layout(app, latestRuleNames)).toEqual(["Em1*"]);
    await app.press("Escape");

    await app.press("g", "#");
    await app.row("Latest rule").click();
    await expect.poll(() => layout(app, latestRuleNames)).toEqual(["cm1", "Em2*"]);
    await app.press("Escape");

    await gmail.modify(A, s.message("plan", 11).id, ["SPAM"], ["INBOX"], { poll: true });
    await app.press("g", "i");
    await app.row("Q4 plan").click();
    await expect.poll(() => layout(app)).toEqual(["c1", "fold7", "c9", "E10", "E12*"]);
    await app.press("Escape");
    await app.press("g", "!");
    await app.row("Q4 plan").click();
    await app.press("Shift+O");
    await expect(app.messages).toHaveCount(12);
    await app.press("Escape");

    await app.press("g", "i");
    await app.row("Person 4").click();
    await expect(app.messages).toHaveCount(1);
    await app.reply("r");
    await app.type("Draft words");
    await app.settle();
    await app.press("Escape", "Escape");
    await expect(app.editor).toHaveCount(0);
    await gmail.poll();
    await app.settle();
    await expect(app.messages).toHaveCount(1);
    const stub = app.conversation.getByRole("button", { name: /Draft/ });
    await expect(stub).toHaveCount(1);
    await expect(stub).not.toHaveAttribute("data-message");
    await expect(stub).not.toHaveAttribute("aria-expanded", "true");
  });

  test("CONV-12 Tab cycles links, Enter opens one outside the app, Esc clears it", async ({
    app,
    gmail,
  }) => {
    await seeds.standard(gmail, app);
    await app.row("Links").click();
    const body = app.messageBody(0);
    const linkA = body.getByRole("link", { name: "Link A" });
    const mailX = body.getByRole("link", { name: "Mail X" });
    await expect(linkA).toBeVisible();
    const url = app.page.url();

    await app.press("Tab");
    await expect(linkA).toHaveClass(/sm-link-focus/);
    await app.press("Tab");
    await expect(mailX).toHaveClass(/sm-link-focus/);
    await expect(linkA).not.toHaveClass(/sm-link-focus/);
    await app.press("Shift+Tab");
    await expect(linkA).toHaveClass(/sm-link-focus/);
    await expect(mailX).not.toHaveClass(/sm-link-focus/);

    await app.press("Enter");
    await app.settle();
    expect(app.page.url()).toBe(url);
    await expect(app.editor).toHaveCount(0);

    await app.press("Escape");
    await expect(body.locator(".sm-link-focus")).toHaveCount(0);
    await expect(app.conversationTitle).toHaveText("Links");
    await app.press("Escape");
    await expect(app.conversationTitle).toHaveCount(0);
    await expect(app.list).toBeVisible();

    await expect
      .poll(async () => (await gmail.platform()).openedUrls)
      .toEqual(["https://example.org/a"]);
  });

  test("CONV-13 shortcuts still work with focus inside a message body", async ({ app, gmail }) => {
    const s = await seeds.standard(gmail, app);
    await app.row("Person 4").click();
    const body = app.messageBody(0).locator("body");
    await expect(body).toContainText("Hello from Sol.");

    await body.click();
    await app.press("?");
    await expect(app.sheet("Keyboard shortcuts")).toBeVisible();
    await app.press("Escape");
    await expect(app.sheet("Keyboard shortcuts")).toHaveCount(0);

    await body.click();
    await app.press("e");
    await expect(app.conversationTitle).toHaveText("Person 5");
    await app.settle();
    expect(await app.gmail.labelsOf(A, s.message("p4", 1).id)).not.toContain("INBOX");
  });

  test("CONV-14 message iframes size to their content", async ({ app, gmail }) => {
    await seeds.standard(gmail, app);
    await app.row("Contract").click();
    await expect(app.messageBody(0).getByText("Please review the contract.")).toBeVisible();
    await app.press("Shift+O");
    await expect(app.messageBody(0).getByText("The first draft of the contract.")).toBeVisible();

    const frames = app.conversation.locator("iframe[data-frame]");
    await expect
      .poll(() =>
        frames.evaluateAll((els) =>
          els.map((f) => {
            const doc = f instanceof HTMLIFrameElement ? f.contentDocument : null;
            const scroller = doc?.scrollingElement;
            if (!doc || !scroller) return "no document";
            const height = f.getBoundingClientRect().height;
            const content = doc.documentElement.scrollHeight;
            if (height + 1 < content) return `frame ${height}px < content ${content}px`;
            if (scroller.scrollHeight > scroller.clientHeight + 1) return "scrolls inside";
            return "fits";
          }),
        ),
      )
      .toEqual(["fits"]);
  });

  test("CONV-15 a conversation left before it loaded is not marked read", async ({
    app,
    gmail,
  }) => {
    const s = await seeds.standard(gmail, app);
    await app.page.clock.pauseAt(await app.now());
    await app.press("j", "j", "j", "j");
    await expect(app.highlighted).toContainText("Person 5");
    await expect(app.row("Person 5")).toHaveAttribute("data-unread");
    const release = await app.park("submit");
    await app.press("Enter", "Escape");
    await expect(app.list).toBeVisible();
    await release();
    await app.page.clock.resume();
    await app.page.clock.setFixedTime(await app.now());
    await app.settle();
    await expect(app.highlighted).toContainText("Person 5");
    await expect(app.row("Person 5")).toHaveAttribute("data-unread");
    expect(await app.gmail.labelsOf(A, s.message("p5", 1).id)).toContain("UNREAD");
  });

  test("CONV-16 the conversation that opens after an action is marked read", async ({
    app,
    gmail,
  }) => {
    const s = await seeds.standard(gmail, app);
    await expect(app.row("Person 5")).toHaveAttribute("data-unread");
    await app.press("j", "j", "j");
    await expect(app.highlighted).toContainText("Person 4");
    await app.press("Enter");
    await expect(app.conversationTitle).toHaveText(s.thread("p4").subject);
    await app.press("e");
    await expect(app.conversationTitle).toHaveText(s.thread("p5").subject);
    await app.settle();
    expect(await app.gmail.labelsOf(A, s.message("p5", 1).id)).not.toContain("UNREAD");
    await app.press("Escape");
    await expect(app.row("Person 5")).not.toHaveAttribute("data-unread");
  });

  test("CONV-17 message headers name people as Superhuman does, and the selected message is a raised card", async ({
    app,
    gmail,
  }) => {
    await seeds.standard(gmail, app);
    const header = () => app.selectedMessage.locator('button[aria-expanded="true"]');

    await app.row("Offsite venue").click();
    await expect(header()).toContainText("Carol Chen to dave, erin & Me");
    await app.press("Escape");
    await app.row("Guild meetup").click();
    await expect(header()).toContainText("Gus Grant to hal & Me");
    await app.press("Escape");
    await app.row("Person 1").click();
    await expect(header()).toContainText("Pia");
    await expect(header()).not.toContainText(" to ");
    await app.press("Escape");

    await app.row("Budget draft").click();
    await expect(header()).toContainText("Me to Kim & lee");
    const [kim, ana] = [app.messages.first(), app.messages.last()];
    await expect(kim).toContainText("Kim Kato");
    const raised = (l: typeof kim) => l.evaluate((el) => getComputedStyle(el).boxShadow !== "none");
    const textLeft = () =>
      ana.locator('button[aria-expanded="true"]').evaluate((el) => el.getBoundingClientRect().left);
    expect(await raised(ana)).toBe(true);
    expect(await raised(kim)).toBe(false);
    const before = await textLeft();
    await app.press("p");
    await expect(kim).toHaveAttribute("aria-current", "true");
    expect(await raised(kim)).toBe(true);
    expect(await raised(ana)).toBe(false);
    expect(await textLeft()).toBe(before);
  });

  test("CONV-18 the subject row's Done and Snooze buttons act like E and H", async ({
    app,
    gmail,
  }) => {
    const s = await seeds.standard(gmail, app);
    await app.row("Person 1").click();
    await expect(app.conversationTitle).toHaveText("Person 1");
    await app.conversation.getByRole("button", { name: "Mark Done (E)" }).click();
    await expect(app.conversationTitle).toHaveText("Person 2");
    await app.settle();
    expect(await app.gmail.labelsOf(A, s.message("p1", 1).id)).not.toContain("INBOX");

    await app.conversation.getByRole("button", { name: "Snooze (H)" }).click();
    await expect(app.page.getByRole("dialog", { name: /^Snooze until/ })).toBeVisible();
  });

  test("CONV-19 a message's dark-mode styles stay off when macOS is in dark mode", async ({
    app,
    gmail,
  }) => {
    await seeds.standard(gmail, app);
    await app.page.emulateMedia({ colorScheme: "dark" });
    await gmail.deliver({
      account: A,
      from: "Calendar <calendar@corp.test>",
      subject: "Updated invitation",
      html:
        "<style>.primary-text{color:#3c4043}" +
        "@media (prefers-color-scheme: dark){.primary-text{color:#e8eaed !important}}</style>" +
        '<p class="primary-text">Itinerary for Tuesday</p>',
    });
    await app.settle();
    await app.row("Updated invitation").click();
    const text = app.conversation
      .locator("iframe[data-frame]")
      .last()
      .contentFrame()
      .getByText("Itinerary for Tuesday");
    await expect(text).toHaveCSS("color", "rgb(60, 64, 67)");
  });

  test("CONV-20 a conversation opened, closed, and opened again in one batch still reads its body", async ({
    app,
    gmail,
  }) => {
    await seeds.standard(gmail, app);
    await gmail.fault(
      { api: "threads.get", query: { format: "full" } },
      { kind: "status", status: 500 },
      { times: 1 },
    );
    await gmail.deliver({
      account: A,
      from: seeds.PERSONS[0] ?? "",
      subject: "Opened twice",
      text: "Read after opening it again.",
      labels: ["INBOX", "UNREAD", "CATEGORY_PERSONAL"],
    });
    await app.settle();
    await expect(app.row("Opened twice")).toHaveCount(1);
    const fullReads = async () =>
      (await gmail.requests("threads.get", { account: A })).filter(
        (r) => r.params.format === "full",
      ).length;
    const before = await fullReads();

    await expect(app.highlighted).toContainText("Person 1");
    await app.page.clock.pauseAt(await app.now());
    const release = await app.park("submit");
    await app.press("Enter");
    await expect(app.conversationTitle).toHaveText("Person 1");
    await app.press("Escape", "k");
    await expect(app.highlighted).toContainText("Opened twice");
    await app.press("Enter");
    await expect(app.list).toHaveCount(0);
    await app.press("Escape");
    await expect(app.highlighted).toContainText("Opened twice");
    await app.press("Enter");
    await expect(app.list).toHaveCount(0);
    await release();
    await app.page.clock.resume();
    await app.page.clock.setFixedTime(await app.now());
    await app.settle();

    await expect(app.conversationTitle).toHaveText("Opened twice");
    await expect(app.messageBody(0).locator("body")).toContainText("Read after opening it again.");
    expect(await fullReads()).toBe(before + 1);
  });
});

test("CONV-21 a message still downloading shows its snippet and a loading bar, then its body, growing in one step", async ({
  app,
  gmail,
  page,
}) => {
  await seeds.standard(gmail, app);
  const body = await gmail.hold({ api: "threads.get", query: { format: "full" } });
  await gmail.deliver({
    account: A,
    from: seeds.PERSONS[0] ?? "",
    subject: "Slow body",
    text: `The first lines arrive before the rest. ${"Then the rest follows, line after line. ".repeat(40)}`,
  });
  await expect
    .poll(async () => (await gmail.requests("threads.get")).some((r) => r.held))
    .toBe(true);
  await app.row("Slow body").click();
  await expect(app.conversationTitle).toHaveText("Slow body");

  const loading = app.conversation.locator("[data-message] [aria-busy=true]");
  await expect(loading).toContainText("The first lines arrive before the rest.");
  await expect(app.conversation.locator("iframe[data-frame]")).toHaveCount(0);

  const snippet = loading.locator("p");
  const snippetStyle = await snippet.evaluate((p) => {
    const s = getComputedStyle(p);
    const top = p.getBoundingClientRect().top;
    const from = p.closest("[data-message]")?.getBoundingClientRect().top ?? 0;
    return { size: s.fontSize, line: s.lineHeight, color: s.color, top: top - from };
  });

  const message = app.conversation.locator("[data-message]").last();
  await message.evaluate((el) => {
    const heights = [el.getBoundingClientRect().height];
    Reflect.set(window, "__heights", heights);
    new MutationObserver(() => heights.push(el.getBoundingClientRect().height)).observe(el, {
      subtree: true,
      childList: true,
      attributes: true,
    });
  });

  await body.release();
  const frame = app.conversation.locator("iframe[data-frame]");
  await expect(frame.contentFrame().locator("body")).toContainText(
    "The first lines arrive before the rest.",
  );
  await expect(loading).toHaveCount(0);

  const heights = await page.evaluate((): number[] => {
    const log: unknown = Reflect.get(window, "__heights");
    return Array.isArray(log) ? log.filter((h) => typeof h === "number") : [];
  });
  expect(heights.length).toBeGreaterThan(0);
  for (let i = 1; i < heights.length; i++) {
    expect(heights[i], `height ${i} of ${heights.join(", ")}`).toBeGreaterThanOrEqual(
      heights[i - 1] ?? 0,
    );
  }

  const bodyStyle = await frame
    .contentFrame()
    .locator("body")
    .evaluate((b) => {
      const s = getComputedStyle(b);
      return { size: s.fontSize, line: s.lineHeight, color: s.color };
    });
  expect({ size: snippetStyle.size, line: snippetStyle.line, color: snippetStyle.color }).toEqual(
    bodyStyle,
  );
  const frameTop = await frame.evaluate((f) => {
    const from = f.closest("[data-message]")?.getBoundingClientRect().top ?? 0;
    return f.getBoundingClientRect().top - from;
  });
  expect(frameTop).toBe(snippetStyle.top);
});

test("CONV-22 a message labelled with a legacy charset but served as UTF-8 reads correctly", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  await gmail.deliver({
    account: A,
    from: seeds.PERSONS[0] ?? "",
    subject: "Знакомство",
    text: "Добрый день, Дина",
    html: "<p>Добрый день, Дина</p>",
    charset: "koi8-r",
  });
  await app.row("Знакомство").click();

  const body = app.conversation.locator("iframe[data-frame]").contentFrame().locator("body");
  await expect(body).toContainText("Добрый день, Дина");
  await expect(body).not.toContainText("п■п╬п╠");
});

test("CONV-23 an address added to the account in Gmail is Me in a conversation already open", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  const header = () => app.selectedMessage.locator('button[aria-expanded="true"]');
  await app.row("Offsite venue").click();
  await expect(header()).toContainText("Carol Chen to dave, erin & Me");

  await gmail.sendAs(A, "dave@corp.test", "Dave");
  await app.focus(false);
  await app.focus(true);
  await app.settle();
  await expect(header()).toContainText("Carol Chen to erin & Me");
});

test("CONV-24 r, Enter, and f do nothing in a conversation that is gone", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  await app.row("Person 3").click();
  await expect(app.conversationTitle).toHaveText("Person 3");
  await app.settle();
  await gmail.deleteThread(A, s.thread("p3").threadId, { poll: true });
  await app.settle();
  await expect(app.conversation.getByText("This conversation is gone.")).toBeVisible();

  await app.press("r", "Enter", "f");
  await app.settle();
  await expect(app.hintBar).toHaveText(/^E to Mark Done\s*·\s*⌘K for commands$/);
  await app.press("Escape");
  await expect(app.list).toBeVisible();
  await expect(app.highlighted).toContainText("Person 4");
  await app.press("j");
  await expect(app.highlighted).toContainText("Person 5");
  await app.settle();
  expect((await gmail.gmail(A)).drafts).toEqual([]);
});

test("CONV-25 a reply open when its conversation turns gone closes as a second Esc would", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  const p3 = s.thread("p3").threadId;
  await app.row("Person 3").click();
  await expect(app.conversationTitle).toHaveText("Person 3");
  await app.reply("r");
  await app.type("Words to keep");
  await gmail.deleteThread(A, p3, { poll: true });
  await app.settle();
  await expect(app.conversation.getByText("This conversation is gone.")).toBeVisible();

  await expect(app.hintBar).toHaveText(/^E to Mark Done\s*·\s*⌘K for commands$/);
  await app.press("Meta+Enter");
  await app.advance(10_000);
  await app.press("Escape");
  await expect(app.list).toBeVisible();
  await expect(app.highlighted).toContainText("Person 4");
  await app.settle();
  const sent = (await gmail.gmail(A)).messages.filter((m) => m.labels.includes("SENT"));
  expect(sent.map((m) => m.threadId)).not.toContain(p3);
  await app.press("g", "d");
  await expect(app.row("Words to keep")).toHaveCount(1);
});

test("CONV-26 a conversation still downloading follows Gmail: a reply joins it, and a deleted thread shows as gone", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  const errors = (await app.errorToastsShown()).length;
  const from = seeds.PERSONS[0] ?? "";
  const heldBodies = async () =>
    (await gmail.requests("threads.get")).some((r) => r.held && r.params.format === "full");

  let body = await gmail.hold({ api: "threads.get", query: { format: "full" } });
  const first = await gmail.deliver({
    account: A,
    from,
    subject: "Slow body",
    text: "Message one.",
  });
  await expect.poll(heldBodies).toBe(true);
  await app.row("Slow body").click();
  await expect(app.conversationTitle).toHaveText("Slow body");
  await expect(app.conversation.locator("[data-message] [aria-busy=true]")).toHaveCount(1);
  await gmail.deliver({
    account: A,
    from,
    subject: "Re: Slow body",
    text: "Message two.",
    threadId: first.threadId,
    inReplyTo: first.messageId,
  });
  await app.settle();
  await gmail.release({ id: body.id });
  await app.settle();
  await expect(app.messages).toHaveCount(2);
  const frames = app.conversation.locator("iframe[data-frame]");
  await expect(frames.first().contentFrame().locator("body")).toContainText("Message one.");
  await expect(frames.last().contentFrame().locator("body")).toContainText("Message two.");
  await app.press("Escape");

  body = await gmail.hold({ api: "threads.get", query: { format: "full" } });
  const doomed = await gmail.deliver({ account: A, from, subject: "Doomed", text: "Soon gone." });
  await expect.poll(heldBodies).toBe(true);
  await app.row("Doomed").click();
  await expect(app.conversationTitle).toHaveText("Doomed");
  await gmail.deleteThread(A, doomed.threadId, { poll: true });
  await app.settle();
  await expect(app.conversation.getByText("This conversation is gone.")).toBeVisible();
  await gmail.release({ id: body.id });
  await app.settle();
  await expect(app.conversation.getByText("This conversation is gone.")).toBeVisible();
  await app.expectNoErrorToasts(errors);
});

test("CONV-27 z while the conversation that opened after e still loads undoes the e, and that conversation opens and is read", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  await app.page.clock.pauseAt(await app.now());
  await app.press("j", "j", "j");
  await expect(app.highlighted).toContainText("Person 4");
  await app.press("Enter");
  await expect(app.conversationTitle).toHaveText(s.thread("p4").subject);

  const release = await app.park("submit");
  await app.press("e");
  await expect(app.conversationTitle).toHaveText(s.thread("p5").subject);
  await expect(app.messages).toHaveCount(0);
  await app.press("z");
  await release();
  await app.page.clock.resume();
  await app.page.clock.setFixedTime(await app.now());
  await app.settle();

  await expect(app.conversationTitle).toHaveText(s.thread("p5").subject);
  expect(await app.gmail.labelsOf(A, s.latest("p4").id)).toContain("INBOX");
  expect(await app.gmail.labelsOf(A, s.message("p5", 1).id)).not.toContain("UNREAD");
  await app.press("Escape");
  await expect(app.row("Person 4")).toHaveCount(1);
  await expect(app.row("Person 5")).not.toHaveAttribute("data-unread");
  await app.expectNoErrorToasts();
});

test("CONV-28 a conversation still loading when the app quits stays unread", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  await app.page.clock.pauseAt(await app.now());
  await app.press("j", "j", "j", "j");
  await expect(app.highlighted).toContainText("Person 5");
  await expect(app.row("Person 5")).toHaveAttribute("data-unread");
  const submit = /\/invoke\/submit$/;
  await app.page.route(submit, () => undefined);
  await app.press("Enter");
  await expect(app.list).toHaveCount(0);
  await app.page.clock.resume();
  await app.quit();
  await app.page.unroute(submit);
  await app.launch();

  await expect(app.row("Person 5")).toHaveAttribute("data-unread");
  expect(await app.gmail.labelsOf(A, s.message("p5", 1).id)).toContain("UNREAD");
  await app.press("j", "j", "j", "j", "Enter");
  await expect(app.conversationTitle).toHaveText(s.thread("p5").subject);
  await app.settle();
  expect(await app.gmail.labelsOf(A, s.message("p5", 1).id)).not.toContain("UNREAD");
});

test("CONV-29 a reply written below the quoted mail is shown, not folded away with the quote", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  const p1 = s.message("p1", 1);
  await gmail.deliver({
    account: A,
    from: seeds.PERSONS[0] ?? "",
    subject: "Re: Person 1",
    html:
      '<div class="moz-cite-prefix">On 02/11/26 09:00, Ana wrote:<br></div>' +
      '<blockquote type="cite"><p>The earlier question</p></blockquote>' +
      "<p>Answer written below the quote</p>",
    threadId: p1.threadId,
    inReplyTo: p1.messageId,
  });
  await app.settle();
  await app.openAndSettle("Person 1");
  const body = app.messageBody(1);
  await expect(body.getByText("Answer written below the quote")).toBeVisible();
  await expect(body.getByText("The earlier question")).toBeVisible();
});
