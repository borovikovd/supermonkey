// The inline reply composer: opening it, addressing, quoting, forwarding,
// which draft each key resumes, and drafts that come from Gmail
// (COVERAGE 5.11; SPEC Reply composer, Addressing).
import type { App } from "./app.ts";
import type { Control, GmailMessage } from "./control.ts";
import { files } from "./files.ts";
import { expect, test } from "./fixtures.ts";
import { attachments, findPart, htmlOf, parseMime } from "./mime.ts";
import * as seeds from "./seeds.ts";
import { A, A_ALIAS, PERSONS, dates } from "./seeds.ts";

// ------------------------------------------------------------------ helpers

/** Every address in a header, lower-cased, in order. */
function emails(header: string | undefined): string[] {
  return [...(header ?? "").matchAll(/[\w.+-]+@[\w-]+(?:\.[\w-]+)+/g)].map((m) =>
    m[0].toLowerCase(),
  );
}

/** Mail this app sent: SENT and carrying its draft header. */
async function sentByApp(gmail: Control, account: string): Promise<GmailMessage[]> {
  const { messages } = await gmail.gmail(account);
  return messages.filter(
    (m) => m.labels.includes("SENT") && m.headers["x-ubermensch-draft"] !== undefined,
  );
}

/** The one message the app sent whose HTML contains `marker`. */
async function sentWith(gmail: Control, account: string, marker: string): Promise<GmailMessage> {
  const found = (await sentByApp(gmail, account)).filter((m) =>
    htmlOf(parseMime(m.raw)).includes(marker),
  );
  expect(found, `one sent message with "${marker}"`).toHaveLength(1);
  const [m] = found;
  if (!m) throw new Error(`no sent message with ${marker}`);
  return m;
}

/** Opens a conversation by clicking its row. */
async function open(app: App, subject: string): Promise<void> {
  await app.row(subject).first().click();
  await expect(app.conversationTitle).toHaveText(subject);
  await app.settle();
}

/** Runs `step` for each item, one after another. */
async function inOrder<T>(
  items: readonly T[],
  step: (item: T, i: number) => Promise<void>,
): Promise<void> {
  await items.reduce<Promise<void>>(
    (prev, item, i) => prev.then(() => step(item, i)),
    Promise.resolve(),
  );
}

/** Highlights a list row with `j` without opening it. */
async function highlight(app: App, subject: string): Promise<void> {
  await expect(app.row(subject)).toHaveCount(1);
  const down = async (tries: number): Promise<void> => {
    if ((await app.highlighted.textContent())?.includes(subject)) return;
    if (tries === 0) throw new Error(`could not highlight ${subject}`);
    await app.press("j");
    await down(tries - 1);
  };
  await down(40);
}

/** Sends the composer on screen once its autosave has run, and lets the outbox send it. */
async function send(app: App): Promise<void> {
  await app.settle();
  await app.press("Meta+Enter");
  await expect(app.editor).toHaveCount(0);
  await app.advance(10_000);
  await app.settle();
}

/** Closes the composer as two `Esc` presses do. */
async function closeComposer(app: App): Promise<void> {
  await app.press("Escape", "Escape");
  await expect(app.editor).toHaveCount(0);
  await app.settle();
}

/** The composer's quote block, opened with `•••`; its text. */
async function quoteText(app: App): Promise<string> {
  await app.page.getByTitle("Show quoted text").click();
  const body = app.page.locator('iframe[title="quoted"]').contentFrame().locator("body");
  await expect(body).not.toBeEmpty();
  return (await body.innerText()).replace(/\s+/g, " ").trim();
}

/** The From button (shown only with more than one sendAs address). */
function fromButton(app: App) {
  return app.page
    .locator("div")
    .filter({ has: app.page.getByText("From", { exact: true }) })
    .last()
    .getByRole("button");
}

/** Addresses of the chips in a field. */
async function chipAddresses(app: App, field: "To" | "Cc" | "Bcc"): Promise<string[]> {
  return app.chips(field).evaluateAll((els) => els.map((el) => el.getAttribute("data-chip") ?? ""));
}

/** Draft stubs in the open conversation. */
function stubs(app: App) {
  return app.conversation.getByRole("button", { name: /^Draft/ });
}

// ------------------------------------------------------------------ cases

test("REP-01 r opens the reply composer under the thread, ready to type", async ({
  app,
  gmail,
}) => {
  // Short enough that the thread does not fit: the composer opens below the fold
  // unless the view scrolls to it.
  await app.page.setViewportSize({ width: 1280, height: 500 });
  await seeds.standard(gmail, app);
  await open(app, "Q4 plan");
  await app.reply("r");
  await app.type("Thanks");

  const composer = app.editor.locator(
    "xpath=ancestor::div[.//button[normalize-space(.)='Send']][1]",
  );
  await expect(app.editor).toHaveText("Thanks");
  // Scrolled into view, it can sit a fraction of a pixel short: scroll offsets are
  // whole pixels and the card's height is not.
  await expect(composer).toBeInViewport({ ratio: 0.99 });
  // Below m12, the last message.
  const last = await app.messages.last().boundingBox();
  const box = await composer.boundingBox();
  if (!last || !box) throw new Error("m12 or the composer has no box");
  expect(box.y).toBeGreaterThanOrEqual(last.y + last.height);
  // The selection bar is on m12, the target.
  await expect(app.messages.last()).toHaveAttribute("aria-current", "true");
  await expect(app.selectedMessage).toHaveCount(1);
  expect(await chipAddresses(app, "To")).toEqual(["dave@corp.test"]);
  await expect(
    app.page.getByText("⌘Enter to Send", { exact: true }).locator("xpath=../.."),
  ).toHaveText(/⌘Enter to Send\s*·\s*Esc to close\s*·\s*⌘K for commands/);

  // The thread is still rendered above, and scrolling up shows it.
  await expect(app.conversationTitle).toHaveText("Q4 plan");
  await expect(app.messages.first()).toContainText("Carol Chen");
  const box2 = await composer.boundingBox();
  await app.page.mouse.move((box2?.x ?? 0) + 10, (box2?.y ?? 0) + 10);
  await app.page.mouse.wheel(0, -10_000);
  await expect(app.conversationTitle).toBeInViewport();
  await expect(app.messages.first()).toBeInViewport();
});

test("REP-02 what r, Enter, and f do on the list and in a conversation", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);

  // On the list, with addr highlighted.
  await highlight(app, "Offsite venue");
  await app.reply("r");
  await expect(app.conversationTitle).toHaveText("Offsite venue");
  expect(await chipAddresses(app, "To")).toEqual(["carol@corp.test"]);
  await expect(app.messages.last()).toHaveAttribute("aria-current", "true");
  await closeComposer(app);
  await app.press("Escape");
  await expect(app.highlighted).toContainText("Offsite venue");

  await app.press("Enter");
  await expect(app.conversationTitle).toHaveText("Offsite venue");
  await app.settle();
  await expect(app.editor).toHaveCount(0);
  await app.press("Escape");
  await expect(app.highlighted).toContainText("Offsite venue");

  await app.reply("f");
  await expect(app.conversationTitle).toHaveText("Offsite venue");
  await expect(app.chips("To")).toHaveCount(0);
  expect(await quoteText(app)).toMatch(/^-+ Forwarded message -+ From: Carol Chen/);
  await closeComposer(app);
  await app.press("Escape");

  // In a conversation, with m9 selected.
  await open(app, "Q4 plan");
  await app.press("p", "p", "p");
  await expect(app.selectedMessage).toContainText("Plan message 9");

  await app.reply("r");
  await expect(app.selectedMessage).toContainText("Plan message 9");
  expect(await chipAddresses(app, "To")).toEqual(["carol@corp.test"]);
  expect(await quoteText(app)).toMatch(/^On .*Carol Chen.* wrote:/);
  await closeComposer(app);
  await expect(app.selectedMessage).toContainText("Plan message 9");

  // Enter is reply all. m9 went To Ana only, so its Cc has no one left once
  // Ana's own addresses are left out (SPEC Addressing).
  await app.reply("Enter");
  await expect(app.selectedMessage).toContainText("Plan message 9");
  expect(await chipAddresses(app, "To")).toEqual(["carol@corp.test"]);
  await expect(app.chips("Cc")).toHaveCount(0);
  expect(await quoteText(app)).toMatch(/^On .*Carol Chen.* wrote:.*Plan message 9/);
  await closeComposer(app);

  await app.reply("f");
  await expect(app.selectedMessage).toContainText("Plan message 9");
  await expect(app.chips("To")).toHaveCount(0);
  expect(await quoteText(app)).toMatch(/^-+ Forwarded message -+ From: Carol Chen.*Plan message 9/);
});

test("REP-03 reply and reply-all addressing", async ({ app, gmail }) => {
  test.slow();
  const s = await seeds.standard(gmail, app);
  // Subjects get `Re:` unless they already start with it (Q5): own's latest
  // message is "Re: Budget draft".
  const rows = [
    {
      key: "addr",
      to: ["carol@corp.test"],
      cc: ["dave@corp.test", "erin@corp.test"],
      from: A,
      subject: "Re: Offsite venue",
    },
    {
      key: "replyto",
      to: ["tickets@help.test"],
      cc: ["jo@help.test"],
      from: A,
      subject: "Re: Ticket #42",
    },
    {
      key: "alias",
      to: ["gus@guild.test"],
      cc: ["hal@guild.test"],
      from: A_ALIAS,
      subject: "Re: Guild meetup",
    },
    {
      key: "own",
      to: ["kim@kite.test"],
      cc: ["lee@kite.test"],
      from: A,
      subject: "Re: Budget draft",
    },
    { key: "ownAlias", to: ["kim@kite.test"], cc: [], from: A_ALIAS, subject: "Re: Alias note" },
    // COVERAGE lists no Cc here, but SPEC Addressing puts the message's other To on Cc.
    {
      key: "news1",
      to: ["news@letters.test"],
      cc: ["subscribers@letters.test"],
      from: A,
      subject: "Re: Digest: week 44",
    },
  ];

  const modes = ["r", "Enter"] as const;
  await inOrder(rows, async (row) => {
    await open(app, s.thread(row.key).subject);
    await inOrder(modes, async (mode) => {
      await app.reply(mode);
      expect(await chipAddresses(app, "To"), `${row.key} ${mode} To`).toEqual(row.to);
      if (mode === "r") await expect(app.field("Cc")).toHaveCount(0);
      else expect(await chipAddresses(app, "Cc"), `${row.key} reply all Cc`).toEqual(row.cc);
      await expect(fromButton(app), `${row.key} ${mode} From`).toContainText(row.from);
      await app.type(`marker-${row.key}-${mode}`);
      await app.settle();
      await app.press("Meta+Enter");
      await expect(app.editor).toHaveCount(0);
    });
    await app.press("Escape");
    await expect(app.conversationTitle).toHaveCount(0);
  });
  await app.advance(10_000);
  await app.settle();

  await inOrder(rows, async (row) => {
    await inOrder(modes, async (mode) => {
      const m = await sentWith(gmail, A, `marker-${row.key}-${mode}`);
      const label = `${row.key} ${mode}`;
      expect(m.threadId, label).toBe(s.thread(row.key).threadId);
      expect(emails(m.headers.from), `${label} From`).toEqual([row.from]);
      expect(emails(m.headers.to), `${label} To`).toEqual(row.to);
      expect(emails(m.headers.cc), `${label} Cc`).toEqual(mode === "r" ? [] : row.cc);
      expect(m.headers.subject, `${label} Subject`).toBe(row.subject);
    });
  });
});

test("REP-03 reply all leaves the To address off Cc", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  // Gus copies himself, so reply all's To (Gus) is also among the message's Cc.
  await gmail.deliver({
    account: A,
    from: "Gus Grant <gus@guild.test>",
    to: [A],
    cc: ["gus@guild.test", "hal@guild.test"],
    subject: "Self copy",
    text: "Copying myself.",
  });
  await app.settle();

  await open(app, "Self copy");
  await app.reply("Enter");
  expect(await chipAddresses(app, "To")).toEqual(["gus@guild.test"]);
  expect(await chipAddresses(app, "Cc")).toEqual(["hal@guild.test"]);
});

test("REP-03 sent to two of the account's addresses: To wins over Cc, then the first (Q3)", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  // The default address is listed first in sendAs, so a sendAs-order pick
  // would answer both from ana@acme.test.
  await gmail.deliver({
    account: A,
    from: "Gus Grant <gus@guild.test>",
    to: [A_ALIAS],
    cc: [A],
    subject: "Alias in To",
    text: "To the alias, Cc the main address.",
  });
  await gmail.deliver({
    account: A,
    from: "Gus Grant <gus@guild.test>",
    to: [A_ALIAS, A],
    subject: "Alias first",
    text: "Both in To, the alias first.",
  });
  await app.settle();
  await inOrder(["Alias in To", "Alias first"], async (subject) => {
    await open(app, subject);
    await app.reply("r");
    await expect(fromButton(app), subject).toContainText(A_ALIAS);
    await closeComposer(app);
    await app.press("Escape");
  });
});

test("REP-04 a sent reply stays on the thread and carries Gmail's quote", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  await open(app, "Offsite venue");
  await app.reply("r");
  await app.type("Sounds good");

  // The quote is a collapsed, read-only block outside the editable document.
  await expect(app.page.locator('iframe[title="quoted"]')).toHaveCount(0);
  const quote = await quoteText(app);
  expect(quote).toMatch(/^On .*Carol Chen.* wrote:.*Which venue should we book/);
  await expect(app.editor).toHaveText("Sounds good");
  await expect(app.editor.locator('iframe, [class*="gmail_quote"]')).toHaveCount(0);

  await send(app);
  const m = await sentWith(gmail, A, "Sounds good");
  const carol = s.message("addr", 1).messageId;
  expect(m.threadId).toBe(s.thread("addr").threadId);
  expect(m.headers["in-reply-to"]).toBe(carol);
  expect(m.headers.references).toContain(carol);
  expect(m.headers["x-ubermensch-draft"]).toBeTruthy();
  const html = htmlOf(parseMime(m.raw));
  const at = html.indexOf('<div class="gmail_quote"');
  expect(at).toBeGreaterThan(-1);
  const quoted = html
    .slice(at)
    .replace(/<[^>]+>/g, " ")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/\s+/g, " ")
    .trim();
  expect(quoted).toMatch(/^On .*Carol Chen.* wrote:.*Which venue should we book/);

  // The reply is the latest message, and no Sending row is left.
  await expect(app.messages).toHaveCount(2);
  await expect(app.messages.last()).toContainText("Ana Ames");
  await expect(app.conversation.getByText("Sending", { exact: true })).toHaveCount(0);
});

test("REP-05 the reply follows the selected message", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  await open(app, "Contract");
  await app.reply("f");
  await app.press("Meta+Shift+o");
  await app.type("kim@kite.test,");
  await expect(app.chips("To")).toHaveCount(1);
  await send(app);
  await expect(app.messages).toHaveCount(2);

  // The forward is now the latest message: r answers Kim.
  await expect(app.messages.last()).toHaveAttribute("aria-current", "true");
  await app.reply("r");
  expect(await chipAddresses(app, "To")).toEqual(["kim@kite.test"]);
  await closeComposer(app);

  // `p` to Alice's message, then r, answers Alice.
  await app.press("p");
  await expect(app.selectedMessage).toContainText("Alice Adams");
  await app.reply("r");
  expect(await chipAddresses(app, "To")).toEqual(["alice@law.test"]);
  expect(await quoteText(app)).toMatch(/^On .*Alice Adams.* wrote:/);
});

test("REP-06 forward quotes the target, carries its attachments, and stays on the thread", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  await open(app, "Contract");
  await app.reply("f");

  await expect(app.chips("To")).toHaveCount(0);
  await expect(fromButton(app)).toContainText(A);
  await expect(fromButton(app)).not.toContainText(A_ALIAS);
  await expect(app.page.getByRole("button", { name: "Remove contract.pdf" })).toHaveCount(1);
  expect(await quoteText(app)).toMatch(
    /^-+ Forwarded message -+ From: Alice Adams <alice@law\.test> Date: .+ Subject: Contract To: .*ana@acme\.test/,
  );

  await app.type("Please see below");
  await app.press("Meta+Shift+o");
  await app.type("kim@kite.test,");
  await send(app);

  const m = await sentWith(gmail, A, "Please see below");
  const alice = s.message("fwd", 1).messageId;
  expect(m.headers.subject).toBe("Fwd: Contract");
  expect(m.threadId).toBe(s.thread("fwd").threadId);
  expect(m.headers["in-reply-to"]).toBe(alice);
  expect(m.headers.references).toContain(alice);
  const tree = parseMime(m.raw);
  const pdf = attachments(tree).find((p) => p.filename === "contract.pdf");
  expect(pdf?.body.equals(files.contractPdf.buffer)).toBe(true);
  const logo = findPart(tree, (p) => p.contentType === "image/png");
  expect(logo?.contentId).toBeTruthy();
  expect(logo?.body.equals(files.logoPng.buffer)).toBe(true);
  const html = htmlOf(tree);
  expect(html).toContain(`cid:${logo?.contentId ?? "missing"}`);
  expect(html).not.toContain("data:image");
});

test("REP-07 which draft r, Enter, and f resume", async ({ app, gmail }) => {
  test.slow();
  const s = await seeds.standard(gmail, app);
  await open(app, "Q4 plan");
  const m12 = app.messages.last();
  const atM12 = () => expect(m12).toHaveAttribute("aria-current", "true");
  const atM9 = () => expect(app.selectedMessage).toContainText("Plan message 9");

  // R1, a reply aimed at m12.
  await app.reply("r");
  await app.type("R1 marker");
  await closeComposer(app);
  await expect(stubs(app)).toHaveCount(1);

  // R1 | m12, r | R1 is resumed.
  await atM12();
  await app.reply("r");
  await atM12();
  await expect(app.editor).toHaveText("R1 marker");
  await closeComposer(app);

  // R1 | m12, Enter | R1 is resumed: Enter is a reply.
  await app.reply("Enter");
  await atM12();
  await expect(app.editor).toHaveText("R1 marker");
  await closeComposer(app);

  // R1 | m12, f | a new forward; R1 stays as a stub.
  await app.reply("f");
  await atM12();
  await expect(app.editor).toHaveText("");
  expect(await quoteText(app)).toMatch(/^-+ Forwarded message -+/);
  await expect(stubs(app)).toHaveCount(1);
  await expect(stubs(app)).toContainText("R1 marker");
  await app.type("F1 marker");
  await closeComposer(app);
  // Every draft shows as a collapsed stub below the last message.
  await expect(stubs(app)).toHaveCount(2);
  const lastBox = await m12.boundingBox();
  if (!lastBox) throw new Error("m12 has no box");
  const tops = await stubs(app).evaluateAll((els) =>
    els.map((el) => el.getBoundingClientRect().top),
  );
  for (const top of tops) expect(top).toBeGreaterThanOrEqual(lastBox.y + lastBox.height);

  // R1 and F1 | m12, f | F1 is resumed.
  await app.reply("f");
  await atM12();
  await expect(app.editor).toHaveText("F1 marker");
  // Clear the thread for the next rows: discard F1 and R1.
  await app.press("Meta+Shift+Comma");
  await expect(app.editor).toHaveCount(0);
  await app.reply("r");
  await expect(app.editor).toHaveText("R1 marker");
  await app.press("Meta+Shift+Comma");
  await expect(app.editor).toHaveCount(0);
  await app.settle();
  await expect(stubs(app)).toHaveCount(0);

  // R2, a reply aimed at m9.
  await app.press("p", "p", "p");
  await atM9();
  await app.reply("r");
  await atM9();
  await app.type("R2 marker");
  await closeComposer(app);

  // R2 | m12, r | a new reply aimed at m12.
  await app.press("n", "n", "n");
  await atM12();
  await app.reply("r");
  await atM12();
  await expect(app.editor).toHaveText("");
  expect(await quoteText(app)).toMatch(/^On .*Dave Diaz.* wrote:.*Plan message 12/);
  await closeComposer(app);
  await expect(stubs(app)).toHaveCount(1);

  // R2 | m9, r | R2 is resumed.
  await app.press("p", "p", "p");
  await atM9();
  await app.reply("r");
  await atM9();
  await expect(app.editor).toHaveText("R2 marker");
  await closeComposer(app);

  // R2 | click R2's stub | R2 is resumed, and m9 becomes selected.
  await app.press("n", "n", "n");
  await atM12();
  await stubs(app).filter({ hasText: "R2 marker" }).click();
  await expect(app.editor).toHaveText("R2 marker");
  await atM9();
  await app.press("Meta+Shift+Comma");
  await expect(app.editor).toHaveCount(0);
  await app.settle();

  // R1 (older) and R3 (newer), both aimed at m12 | m12, r | R3 is resumed.
  await app.press("n", "n", "n");
  await atM12();
  await app.reply("r");
  await app.type("R1 again");
  await closeComposer(app);
  await app.advance(60_000);
  // A second reply aimed at m12 cannot be made with r (it resumes R1), so R3 comes from Gmail.
  await gmail.createDraft(
    A,
    {
      subject: "Re: Q4 plan",
      to: ["dave@corp.test"],
      threadId: s.thread("plan").threadId,
      inReplyTo: s.message("plan", 12).messageId,
      html: "<p>R3 marker</p>",
    },
    { poll: true },
  );
  await app.settle();
  await expect(stubs(app)).toHaveCount(2);
  await atM12();
  await app.reply("r");
  await atM12();
  await expect(app.editor).toHaveText("R3 marker");
});

test("REP-08 a half-written reply survives new replies", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  await open(app, "Person 2");
  await app.reply("r");
  await app.type("half");
  await closeComposer(app);
  await app.press("Escape");

  await gmail.deliver({
    account: A,
    from: PERSONS[0] ?? "",
    to: [A],
    subject: "Re: Person 2",
    text: "Pia joins in.",
    threadId: s.thread("p2").threadId,
    inReplyTo: s.latest("p2").messageId,
  });
  await app.settle();

  await open(app, "Person 2");
  await expect(app.messages).toHaveCount(2);
  await expect(app.messages.last()).toHaveAttribute("aria-current", "true");
  await app.reply("r");
  await expect(app.editor).toHaveText("half");
  // Its target is m1, so m1 is selected.
  await expect(app.messages.first()).toHaveAttribute("aria-current", "true");
  await expect(app.selectedMessage).toContainText("Quin Quill");

  await send(app);
  const m = await sentWith(gmail, A, "half");
  expect(emails(m.headers.to)).toEqual(["quin@people.test"]);
  expect(m.headers["in-reply-to"]).toBe(s.message("p2", 1).messageId);
});

test("REP-09 drafts from Gmail get a kind and a target", async ({ app, gmail }) => {
  test.slow();
  const s = await seeds.drafts(gmail, app);

  // dNew: new. Enter on its Drafts row opens the c screen.
  await app.press("g", "d");
  await highlight(app, "Idea");
  await app.press("Enter");
  await expect(app.page.getByRole("heading", { name: "New Message" })).toBeVisible();
  await expect(app.editor).toHaveText("An idea for later.");
  await closeComposer(app);
  await app.page.getByRole("navigation").getByTitle("Inbox", { exact: true }).click();

  // dFwd: forward, aimed at Alice's message. f resumes it, r does not.
  await open(app, "Contract");
  await app.reply("r");
  await expect(app.editor).toHaveText("");
  await closeComposer(app);
  await app.reply("f");
  await expect(app.editor).toHaveText("FYI");
  await expect(app.messages.first()).toHaveAttribute("aria-current", "true");
  await closeComposer(app);
  await app.press("Escape");

  // dLatest: aimed at p4's latest, so r on the latest resumes it.
  await open(app, "Person 4");
  await app.reply("r");
  await expect(app.editor).toHaveText("My text");
  await closeComposer(app);
  await app.press("Escape");

  // Started on the latest message, a Gmail draft is still resumed after a new
  // arrival. The seed dates dLatest before its target (70 vs 40 minutes ago), so
  // by SPEC it was not started on the latest; this draft is dated after it.
  await app.advance(60_000);
  await gmail.createDraft(
    A,
    {
      subject: "Re: Person 5",
      to: [PERSONS[4] ?? ""],
      threadId: s.thread("p5").threadId,
      inReplyTo: s.latest("p5").messageId,
      html: "<p>Started on the latest</p>",
    },
    { poll: true },
  );
  await app.settle();
  await gmail.deliver({
    account: A,
    from: PERSONS[4] ?? "",
    to: [A],
    subject: "Re: Person 5",
    text: "Tom again.",
    threadId: s.thread("p5").threadId,
    inReplyTo: s.latest("p5").messageId,
  });
  await app.settle();
  await open(app, "Person 5");
  await expect(app.messages).toHaveCount(2);
  await expect(app.messages.last()).toHaveAttribute("aria-current", "true");
  await app.reply("r");
  await expect(app.editor).toHaveText("Started on the latest");
  await closeComposer(app);
  await app.press("Escape");

  // The app first sees this draft after a newer message has arrived. It still
  // counts as started on the latest: its target was the newest message older
  // than the draft, though no longer the thread's latest.
  await gmail.deliver({
    account: A,
    from: PERSONS[2] ?? "",
    to: [A],
    subject: "Re: Person 3",
    text: "Rae again.",
    threadId: s.thread("p3").threadId,
    inReplyTo: s.latest("p3").messageId,
    poll: false,
  });
  await gmail.createDraft(
    A,
    {
      subject: "Re: Person 3",
      to: [PERSONS[2] ?? ""],
      threadId: s.thread("p3").threadId,
      inReplyTo: s.latest("p3").messageId,
      html: "<p>Written before Rae replied</p>",
      date: dates.minutesAgo(5),
    },
    { poll: false },
  );
  await gmail.poll();
  await app.settle();
  await open(app, "Person 3");
  await expect(app.messages).toHaveCount(2);
  await expect(app.messages.last()).toHaveAttribute("aria-current", "true");
  await app.reply("r");
  await expect(app.editor).toHaveText("Written before Rae replied");
  await expect(app.messages.first()).toHaveAttribute("aria-current", "true");
  await closeComposer(app);
  await app.press("Escape");

  // dEarly: aimed at plan's m3. r on m12 starts a new reply; r on m3 resumes it.
  await open(app, "Q4 plan");
  await app.reply("r");
  await expect(app.editor).toHaveText("");
  await closeComposer(app);
  await app.press(..."ppppppppp".split(""));
  await expect(app.selectedMessage).toContainText("Plan message 3");
  await app.reply("r");
  await expect(app.editor).toHaveText("About message three.");
  await closeComposer(app);
  await app.press("Escape");

  // dUnknown: In-Reply-To matches nothing, so it is aimed at p6's latest.
  await open(app, "Person 6");
  await app.reply("r");
  await expect(app.editor).toHaveText("Aimed at nobody.");
  await closeComposer(app);
  await app.press("Escape");

  // The kind sticks: without "Fwd:", dFwd is still a forward.
  const dFwd = s.draft("dFwd");
  await gmail.updateDraft(
    A,
    dFwd.id,
    {
      subject: "Contract",
      threadId: dFwd.threadId,
      inReplyTo: s.message("fwd", 1).messageId,
      html: "<p>FYI edited</p>",
    },
    { poll: true },
  );
  await app.settle();
  await open(app, "Contract");
  await app.reply("r");
  await expect(app.editor).toHaveText("");
  await closeComposer(app);
  await app.reply("f");
  await expect(app.editor).toHaveText("FYI edited");
});

test("REP-10 closing and sending the reply composer keep the conversation open", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  await open(app, "Person 3");
  await app.reply("r");
  await app.type("Kept as a draft");

  // The first Esc blurs the editor; the composer stays.
  await app.press("Escape");
  await expect(app.editor).not.toBeFocused();
  await expect(app.editor).toBeVisible();

  // The second closes it; the conversation stays, with a stub and a Gmail draft.
  await app.press("Escape");
  await app.settle();
  await expect(app.editor).toHaveCount(0);
  await expect(app.conversationTitle).toHaveText("Person 3");
  await expect(stubs(app)).toHaveCount(1);
  const drafts = (await gmail.gmail(A)).drafts;
  expect(drafts).toHaveLength(1);
  expect(htmlOf(parseMime(drafts[0]?.message.raw ?? ""))).toContain("Kept as a draft");

  // r resumes it; Cmd+Enter sends and keeps the conversation open.
  await app.reply("r");
  await expect(app.editor).toHaveText("Kept as a draft");
  await app.press("Meta+Enter");
  await expect(app.editor).toHaveCount(0);
  await expect(app.conversationTitle).toHaveText("Person 3");
  const sending = app.conversation.getByText("Sending", { exact: true });
  await expect(sending).toBeVisible();
  await expect(sending.locator("xpath=..")).toContainText("Me");
  await expect(stubs(app)).toHaveCount(0);

  // n and p never select the Sending row.
  await app.press("n");
  await expect(app.selectedMessage).toHaveCount(1);
  await expect(app.messages.last()).toHaveAttribute("aria-current", "true");
  await app.press("p");
  await expect(app.selectedMessage).toHaveCount(1);
  await expect(app.messages.last()).toHaveAttribute("aria-current", "true");
  await expect(app.messages).toHaveCount(1);

  // After the send, the real message replaces the Sending row.
  await app.advance(10_000);
  await app.settle();
  await expect(sending).toHaveCount(0);
  await expect(app.messages).toHaveCount(2);
  await expect(app.messages.last()).toContainText("Ana Ames");
});

test("REP-11 a draft read from Gmail is split at its gmail_quote", async ({ app, gmail }) => {
  const s = await seeds.drafts(gmail, app);
  await open(app, "Person 4");
  await app.reply("r");
  await expect(app.editor).toHaveText("My text");
  expect(await quoteText(app)).toMatch(/^On .*Sol Snow.* wrote:.*Hello from Sol\./);

  await send(app);
  const sent = (await sentByApp(gmail, A)).filter((m) => m.threadId === s.thread("p4").threadId);
  expect(sent).toHaveLength(1);
  const html = htmlOf(parseMime(sent[0]?.raw ?? ""));
  expect(html).toContain("My text");
  expect(html.match(/class="gmail_quote"/g) ?? []).toHaveLength(1);
});

test("REP-12 a Gmail draft's attachments are downloaded before its first upsert", async ({
  app,
  gmail,
}) => {
  const s = await seeds.drafts(gmail, app);

  /** The draft in Gmail, its HTML, and its spec.pdf part. */
  const inGmail = async (draftId: string) => {
    const d = (await gmail.gmail(A)).drafts.find((x) => x.id === draftId);
    const tree = parseMime(d?.message.raw ?? "");
    return {
      html: htmlOf(tree),
      pdf: attachments(tree).find((p) => p.filename === "spec.pdf"),
    };
  };
  /** attachments.get for the draft's message comes before its first drafts.update. */
  const order = async (draftId: string, messageId: string) => {
    const log = await gmail.requests();
    const get = log.findIndex((r) => r.api === "attachments.get" && r.path.includes(messageId));
    const update = log.findIndex((r) => r.api === "drafts.update" && r.path.includes(draftId));
    expect(get, "attachments.get").toBeGreaterThan(-1);
    expect(update, "drafts.update").toBeGreaterThan(-1);
    expect(get).toBeLessThan(update);
  };

  // Online.
  const dLatest = s.draft("dLatest");
  await open(app, "Person 4");
  await app.reply("r");
  await expect(app.editor).toHaveText("My text");
  await app.type(" more");
  await app.settle();
  await app.eventually(A, (g) =>
    g.drafts.some((d) => d.id === dLatest.id && htmlOf(parseMime(d.message.raw)).includes("more")),
  );
  const online = await inGmail(dLatest.id);
  expect(online.html).toContain("My text more");
  expect(online.pdf?.body.equals(files.specPdf.buffer)).toBe(true);
  await order(dLatest.id, dLatest.messageId);
  await closeComposer(app);
  await app.press("Escape");

  // Offline: the composer still opens; the downloads run before the queued upsert.
  const second = await gmail.createDraft(
    A,
    {
      subject: "Re: Person 5",
      to: [PERSONS[4] ?? ""],
      threadId: s.thread("p5").threadId,
      inReplyTo: s.latest("p5").messageId,
      html: "<p>Second text</p>",
      attachments: [
        {
          filename: files.specPdf.name,
          mimeType: files.specPdf.mimeType,
          content: files.specPdf.buffer.toString("base64"),
          base64: true,
        },
      ],
    },
    { poll: true },
  );
  await app.settle();
  await gmail.offline(true);
  await open(app, "Person 5");
  await app.reply("r");
  await expect(app.editor).toHaveText("Second text");
  await app.type(" more");
  await app.settle();
  await gmail.offline(false);
  await app.settle();
  await app.eventually(A, (g) =>
    g.drafts.some((d) => d.id === second.id && htmlOf(parseMime(d.message.raw)).includes("more")),
  );
  const offline = await inGmail(second.id);
  expect(offline.html).toContain("Second text more");
  expect(offline.pdf?.body.equals(files.specPdf.buffer)).toBe(true);
  await order(second.id, second.messageId);
});

test("REP-13 reply keys pressed before the conversation loads open one composer", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  await highlight(app, "Person 3");
  // Nothing reaches the core, so the conversation stays unloaded.
  const release = await app.park("submit");
  await app.press("r", "f");
  await release();
  await app.settle();
  await expect(app.editor).toHaveCount(1);
  // The later key wins: a forward starts with an empty To.
  await expect(app.chips("To")).toHaveCount(0);

  await closeComposer(app);
  await expect(stubs(app)).toHaveCount(0);
  expect((await gmail.gmail(A)).drafts).toHaveLength(0);

  // A reply key whose conversation was left before it loaded is dropped,
  // even if the conversation is opened again.
  await app.press("Escape");
  await highlight(app, "Person 5");
  const again = await app.park("submit");
  await app.press("r", "Escape", "Enter");
  await again();
  await app.settle();
  await expect(app.conversationTitle).toHaveText("Person 5");
  await expect(app.editor).toHaveCount(0);
});
