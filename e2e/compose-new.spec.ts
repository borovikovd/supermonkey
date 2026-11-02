// The `c` screen, formatting and compose shortcuts, the Esc machine, the
// untouched rule, discard, and what takes a composer off screen
// (COVERAGE 5.12; SPEC Compose, Keyboard, UI state).
import type { App } from "./app.ts";
import type { Control, GmailMessage } from "./control.ts";
import { expect, test } from "./fixtures.ts";
import { htmlOf, parseMime } from "./mime.ts";
import * as seeds from "./seeds.ts";
import { A, A_ALIAS, B } from "./seeds.ts";

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

/** The one message the app sent with this subject. */
async function sentWithSubject(
  gmail: Control,
  account: string,
  subject: string,
): Promise<GmailMessage> {
  const found = (await sentByApp(gmail, account)).filter((m) => m.headers.subject === subject);
  expect(found, `one sent message "${subject}"`).toHaveLength(1);
  const [m] = found;
  if (!m) throw new Error(`no sent message ${subject}`);
  return m;
}

/** Drafts in Gmail whose subject is `subject`. */
async function gmailDrafts(gmail: Control, account: string, subject?: string) {
  const { drafts } = await gmail.gmail(account);
  return subject === undefined
    ? drafts
    : drafts.filter((d) => d.message.headers.subject === subject);
}

/** Visible text of an HTML body. */
const textOf = (html: string) =>
  html
    .replace(/<[^>]+>/g, " ")
    .replace(/\s+/g, " ")
    .trim();

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

/**
 * Waits two frames. TipTap focuses the editor in the next frame, so a field
 * shortcut pressed in the same frame as Cmd+Shift+M is undone by it.
 */
async function frames(app: App): Promise<void> {
  await app.page.evaluate(
    () => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))),
  );
}

/** Cmd+Shift+M, and waits until the editor has focus. */
async function focusBody(app: App): Promise<void> {
  await app.press("Meta+Shift+m");
  await expect(app.editor).toBeFocused();
  await frames(app);
}

/** Cmd+Shift+S, and waits until Subject has focus (it moves in the next frame). */
async function focusSubject(app: App): Promise<void> {
  await app.press("Meta+Shift+s");
  await expect(app.subject).toBeFocused();
}

const newMessage = (app: App) => app.page.getByRole("heading", { name: "New Message" });

/** Fills a fresh `c` composer: To, Subject, and body. Leaves the editor focused. */
async function fill(app: App, to: string, subject: string, body: string): Promise<void> {
  await app.press("Meta+Shift+o");
  await app.type(`${to},`);
  await expect(app.chips("To").last()).toHaveAttribute("data-chip", to);
  await focusSubject(app);
  await app.type(subject);
  await focusBody(app);
  await app.type(body);
}

/** Closes the composer as two `Esc` presses do. */
async function closeComposer(app: App): Promise<void> {
  await app.press("Escape", "Escape");
  await expect(app.editor).toHaveCount(0);
  await app.settle();
}

/** Sends the composer on screen once its autosave has run (see NEW-02 for the bug this avoids). */
async function sendSaved(app: App): Promise<void> {
  await app.settle();
  await app.press("Meta+Enter");
  await expect(app.editor).toHaveCount(0);
}

/** A recipient field by its exact name (`app.field("Cc")` also matches Bcc). */
function field(app: App, label: "To" | "Cc" | "Bcc") {
  return app.page.getByRole("combobox", { name: label, exact: true });
}

/** The From button (shown only with more than one sendAs address). */
function fromButton(app: App) {
  return app.page
    .locator("div")
    .filter({ has: app.page.getByText("From", { exact: true }) })
    .last()
    .getByRole("button");
}

/** Draft stubs in the open conversation. */
function stubs(app: App) {
  return app.conversation.getByRole("button", { name: /^Draft/ });
}

function rail(app: App, title: string) {
  return app.page.getByRole("navigation").getByTitle(title, { exact: true });
}

/**
 * Rows in the Drafts view; returns to Inbox afterwards. Uses the rail: a `g`
 * pressed within 1.5 s of an earlier `g` loses its prefix to that one's reset
 * timer (src/keyboard/keys.ts:250-253).
 */
async function draftRows(app: App): Promise<string[]> {
  await rail(app, "Drafts (g d)").click();
  await expect(rail(app, "Drafts (g d)")).toHaveClass(/bg-bg/);
  await app.settle();
  const rows = await app.subjects();
  await rail(app, "Inbox").click();
  await expect(app.section("Important")).toBeVisible();
  return rows;
}

// ------------------------------------------------------------------ cases

test("NEW-01 c replaces the screen, and closing it returns to where it came from", async ({
  app,
  gmail,
}) => {
  test.slow();
  await seeds.standard(gmail, app);

  const starts: { name: string; go: () => Promise<void>; check: () => Promise<void> }[] = [
    {
      name: "Inbox, row 3",
      go: () => app.press("j", "j"),
      check: async () => {
        await expect(app.list).toBeVisible();
        await expect(app.highlighted).toContainText("Person 3");
      },
    },
    {
      name: "p1 open",
      go: () => open(app, "Person 1"),
      check: async () => {
        await expect(app.conversationTitle).toHaveText("Person 1");
      },
    },
    {
      name: "Starred",
      go: () => app.press("Escape", "g", "s"),
      check: async () => {
        await expect(rail(app, "Starred (g s)")).toHaveClass(/bg-bg/);
        await expect(app.row("Trip ideas")).toHaveCount(1);
        await expect(app.row("Q4 plan")).toHaveCount(0);
      },
    },
  ];

  await inOrder(starts, async (start, i) => {
    await start.go();
    await start.check();

    // Esc Esc with typed content.
    await app.compose();
    await expect(newMessage(app)).toBeVisible();
    await expect(app.field("To")).toBeFocused();
    await expect(app.subject).toBeVisible();
    await expect(app.list).toHaveCount(0);
    await expect(app.conversationTitle).toHaveText("New Message");
    await fill(app, "kim@kite.test", `Kept ${i}`, "Typed content");
    await closeComposer(app);
    await expect(newMessage(app)).toHaveCount(0);
    await start.check();

    // After sending.
    await app.compose();
    await fill(app, "kim@kite.test", `Sent ${i}`, "Sent content");
    await sendSaved(app);
    await expect(newMessage(app)).toHaveCount(0);
    await start.check();
  });
});

test("NEW-02 compose and send a formatted message, including the last keystrokes", async ({
  app,
  gmail,
}) => {
  test.fail(
    true,
    "BUG: text typed within 500 ms before Cmd+Enter is not sent: sendOnScreen calls openComposer.pending() for the recipient check (src/compose/start.ts:244), which clears `dirty`, so flushSave (start.ts:254) finds nothing to save",
  );
  await seeds.standard(gmail, app);
  await app.compose();
  await app.type("kim@kite.test,");
  await focusSubject(app);
  await app.type("Plan");
  await focusBody(app);
  await app.type("Hello ");
  await app.press("Meta+b");
  await app.type("bold");
  await app.press("Meta+b");
  await app.type(" ");
  await app.press("Meta+i");
  await app.type("italic");
  await app.press("Meta+i");
  await app.type(" ");
  await app.press("Meta+u");
  await app.type("underline");
  await app.press("Meta+u");
  await app.type(" ");
  await app.press("Meta+Shift+x");
  await app.type("strike");
  await app.press("Meta+Shift+x");
  await app.press("Shift+Enter");
  await app.type("after break");
  await app.press("Enter", "Meta+Shift+8");
  await app.type("bullet");
  await app.press("Enter", "Enter", "Meta+Shift+7");
  await app.type("numbered");
  await app.press("Enter", "Enter");
  await app.type("see here");
  await app.press(..."here".split("").map(() => "Shift+ArrowLeft"));
  await app.press("Meta+k");
  await expect(app.page.getByPlaceholder("https://")).toBeFocused();
  await app.type("example.org");
  await app.press("Enter");
  await expect(app.editor.locator('a[href*="example.org"]')).toHaveText("here");
  await expect(app.editor).toBeFocused();
  await frames(app);
  await app.press("ArrowRight", "Enter");
  // The last keystrokes, then Send at once.
  await app.type("end");
  await app.press("Meta+Enter");

  await expect(newMessage(app)).toHaveCount(0);
  await expect(app.list).toBeVisible();
  const toast = app.toast("Sending…");
  await expect(toast).toBeVisible();
  await expect(toast.getByRole("button", { name: "Undo" })).toBeVisible();

  await app.advance(10_000);
  await app.settle();
  const m = await sentWithSubject(gmail, A, "Plan");
  expect(emails(m.headers.from)).toEqual([A]);
  expect(emails(m.headers.to)).toEqual(["kim@kite.test"]);
  const html = htmlOf(parseMime(m.raw));
  expect(html).toMatch(/<strong>bold<\/strong>/);
  expect(html).toMatch(/<em>italic<\/em>/);
  expect(html).toMatch(/<u>underline<\/u>/);
  expect(html).toMatch(/<s>strike<\/s>/);
  expect(html).toMatch(/<br\s*\/?>/);
  expect(html).toMatch(/<ul>\s*<li>/);
  expect(html).toMatch(/<ol>\s*<li>/);
  expect(html).toMatch(/<a [^>]*href="[^"]*example\.org[^"]*"[^>]*>here<\/a>/);
  expect(textOf(html)).toMatch(/end$/);

  expect(await gmailDrafts(gmail, A)).toHaveLength(0);
  await app.press("g", "t");
  await expect.poll(() => app.subjects()).toContain("Plan");
});

test("NEW-03 TipTap shortcuts that clash with the compose table are replaced", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  await app.compose();
  await fill(app, "kim@kite.test", "Clash test", "one");

  // Cmd+Shift+B: the Bcc field, not a blockquote.
  await app.press("Meta+Shift+b");
  await expect(field(app, "Bcc")).toBeFocused();
  await expect(app.editor.locator("blockquote")).toHaveCount(0);

  // Cmd+Shift+S: Subject, not strikethrough.
  await focusBody(app);
  await focusSubject(app);
  await focusBody(app);
  await app.type(" two");
  await expect(app.editor.locator("s")).toHaveCount(0);

  // Shift+Enter is a hard break; Cmd+Shift+X is strikethrough.
  await app.press("Shift+Enter");
  await app.type("three ");
  await app.press("Meta+Shift+x");
  await app.type("struck");
  await app.press("Meta+Shift+x");
  await expect(app.editor.locator("br")).toHaveCount(1);
  await expect(app.editor.locator("s")).toHaveText("struck");

  // Cmd+Enter sends, without adding a hard break.
  await sendSaved(app);
  await app.advance(10_000);
  await app.settle();
  const html = htmlOf(parseMime((await sentWithSubject(gmail, A, "Clash test")).raw));
  expect(html.match(/<br\s*\/?>/g) ?? []).toHaveLength(1);
  expect(html).toMatch(/<s>struck<\/s>/);
  expect(html).not.toContain("<blockquote");
  expect(html.match(/<s>/g) ?? []).toHaveLength(1);
});

test("NEW-04 compose field shortcuts work whether the editor is focused or blurred", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  await app.compose();
  await focusBody(app);
  await app.type("Body");

  const rows: [string, () => Promise<void>][] = [
    ["Meta+Shift+o", () => expect(field(app, "To")).toBeFocused()],
    ["Meta+Shift+c", () => expect(field(app, "Cc")).toBeFocused()],
    ["Meta+Shift+b", () => expect(field(app, "Bcc")).toBeFocused()],
    [
      "Meta+Shift+f",
      async () => {
        const picker = app.page.getByRole("dialog", { name: "Send from" });
        await expect(picker).toBeVisible();
        await expect(picker.getByRole("option")).toHaveText([
          new RegExp(A.replace(/\./g, "\\.")),
          new RegExp(A_ALIAS.replace(/\./g, "\\.")),
        ]);
        await app.press("Escape");
        await expect(picker).toHaveCount(0);
      },
    ],
    ["Meta+Shift+s", () => expect(app.subject).toBeFocused()],
    ["Meta+Shift+m", () => expect(app.editor).toBeFocused()],
    [
      "Meta+Shift+a",
      async () => {
        const chooser = app.page.waitForEvent("filechooser", { timeout: 5_000 });
        await app.press("Meta+Shift+a");
        await chooser;
      },
    ],
  ];

  await inOrder(["focused", "blurred"] as const, async (state) => {
    await inOrder(rows, async ([key, check]) => {
      await focusBody(app);
      if (state === "blurred") {
        await app.press("Escape");
        await expect(app.editor).not.toBeFocused();
      }
      await test.step(`${key}, ${state}`, async () => {
        if (key !== "Meta+Shift+a") await app.press(key);
        await check();
        await expect(newMessage(app)).toBeVisible();
      });
    });
  });
});

test("NEW-05 Esc in compose: the first blurs, the second closes; while blurred, only a few keys work", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  const inbox = await app.subjects();
  await app.compose();
  await fill(app, "kim@kite.test", "Blur test", "Body text");

  await app.press("Escape");
  await expect(app.editor).not.toBeFocused();
  await expect(newMessage(app)).toBeVisible();

  // Keys from the shortcut table do nothing while blurred.
  await app.press("e", "j", "#", "x", "s", "z", "!");
  await app.settle();
  await expect(app.editor).toHaveText("Body text");
  await expect(newMessage(app)).toBeVisible();
  expect(await gmail.requests("messages.batchModify")).toEqual([]);
  expect(await gmail.requests("messages.trash")).toEqual([]);

  // Enter refocuses the editor.
  await app.press("Enter");
  await expect(app.editor).toBeFocused();
  await app.type(" more");
  await expect(app.editor).toHaveText("Body text more");
  await app.press("Escape");
  await expect(app.editor).not.toBeFocused();

  // Cmd+K: the draft actions, and Esc closes them; the composer stays, blurred.
  await app.press("Meta+k");
  const actions = app.page.getByRole("dialog", { name: "Draft actions" });
  await expect(actions).toBeVisible();
  await expect(actions.getByRole("option")).toHaveText([
    /^Send/,
    /^Attach a file/,
    /^Discard draft/,
  ]);
  await app.press("Escape");
  await expect(actions).toHaveCount(0);
  await expect(newMessage(app)).toBeVisible();
  await expect(app.editor).not.toBeFocused();

  // The last Esc closes the composer and keeps the draft.
  await app.press("Escape");
  await expect(newMessage(app)).toHaveCount(0);
  await app.settle();
  expect(await app.subjects()).toEqual(inbox);
  await expect(app.selected).toHaveCount(0);
  const [draft] = await gmailDrafts(gmail, A, "Blur test");
  expect(htmlOf(parseMime(draft?.message.raw ?? ""))).toContain("Body text more");
  expect(await draftRows(app)).toEqual(["Blur test"]);

  // Variant: while blurred, Cmd+Enter sends.
  await rail(app, "Drafts (g d)").click();
  await app.row("Blur test").click();
  await expect(newMessage(app)).toBeVisible();
  await expect(app.editor).toHaveText("Body text more");
  await app.settle();
  await app.press("Escape");
  await expect(app.editor).not.toBeFocused();
  await app.press("Meta+Enter");
  await expect(newMessage(app)).toHaveCount(0);
  await app.advance(10_000);
  await app.settle();
  await sentWithSubject(gmail, A, "Blur test");
});

test("NEW-06 shortcuts never fire while typing in compose", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  const inbox = await app.subjects();
  const text = "Je suis #1! E? K/O x z";
  // Person 2 is read, so opening it makes no Gmail call of its own.
  await open(app, "Person 2");
  await app.reply("r");
  await app.type(text);
  await expect(app.editor).toHaveText(text);

  await focusSubject(app);
  await app.press("Meta+a");
  await app.type(text);
  await expect(app.subject).toHaveValue(text);

  await app.press("Meta+Shift+o");
  await expect(app.field("To")).toBeFocused();
  await app.type(text);
  await expect(app.field("To")).toHaveValue(text);

  await app.settle();
  await expect(app.conversationTitle).toHaveText("Person 2");
  await expect(app.editor).toHaveText(text);
  expect(await gmail.requests("messages.batchModify")).toEqual([]);
  expect(await gmail.requests("messages.trash")).toEqual([]);
  await focusBody(app);
  await closeComposer(app);
  await app.press("Escape");
  await expect(app.list).toBeVisible();
  expect(await app.subjects()).toEqual(inbox);
  await expect(app.selected).toHaveCount(0);
});

test("NEW-07 a fresh composer closed untouched leaves nothing behind", async ({ app, gmail }) => {
  test.slow();
  await seeds.standard(gmail, app);
  let kept = 0;

  /** Local stubs, Drafts rows, and Gmail drafts after a row. */
  const expectLeft = async (row: string, stubCount: number | null) => {
    await app.settle();
    if (stubCount !== null) await expect(stubs(app), `${row}: stubs`).toHaveCount(stubCount);
    expect(await gmailDrafts(gmail, A), `${row}: Gmail drafts`).toHaveLength(kept);
    if (stubCount !== null) await app.press("Escape");
    expect(await draftRows(app), `${row}: Drafts rows`).toHaveLength(kept);
  };

  // Untouched rows: nothing left.
  await open(app, "Person 1");
  await app.reply("r");
  await closeComposer(app);
  await expectLeft("r, untouched", 0);

  await open(app, "Offsite venue");
  await app.reply("Enter");
  await closeComposer(app);
  await expectLeft("Enter, untouched", 0);

  await open(app, "Contract");
  await app.reply("f");
  await expect(app.page.getByRole("button", { name: "Remove contract.pdf" })).toHaveCount(1);
  await closeComposer(app);
  await expectLeft("f, untouched", 0);

  await app.compose();
  await closeComposer(app);
  await expectLeft("c, untouched", null);

  await open(app, "Person 2");
  await app.reply("r");
  await app.press("Control+2");
  await expect(app.row("B 2")).toHaveCount(1);
  await app.settle();
  expect(await gmailDrafts(gmail, B)).toHaveLength(0);
  await app.press("Control+1");
  await expect(app.row("Person 2")).toHaveCount(1);
  await expectLeft("r, closed by Ctrl+2", null);

  // Touched rows: kept.
  await open(app, "Person 1");
  await app.reply("r");
  await app.type("Typed body");
  await closeComposer(app);
  kept++;
  await expectLeft("r, typed", 1);

  await open(app, "Person 2");
  await app.reply("r");
  await app.press("Meta+Shift+o");
  await app.type("kai@nowhere.test,");
  await expect(app.chips("To")).toHaveCount(2);
  await focusBody(app);
  await closeComposer(app);
  kept++;
  await expectLeft("r, To chip added", 1);

  await open(app, "Person 3");
  await app.reply("r");
  await expect(fromButton(app)).toContainText(A);
  await app.press("Meta+Shift+f");
  await app.page
    .getByRole("dialog", { name: "Send from" })
    .getByRole("option", { name: new RegExp(A_ALIAS.replace(/\./g, "\\.")) })
    .click();
  await expect(fromButton(app)).toContainText(A_ALIAS);
  await focusBody(app);
  await closeComposer(app);
  kept++;
  await expectLeft("r, From changed", 1);

  await open(app, "Contract");
  await app.reply("f");
  await app.page.getByRole("button", { name: "Remove contract.pdf" }).click();
  await expect(app.page.getByRole("button", { name: "Remove contract.pdf" })).toHaveCount(0);
  await focusBody(app);
  await closeComposer(app);
  kept++;
  await expectLeft("f, attachment removed", 1);

  await app.compose();
  await focusSubject(app);
  await app.type("Subject only");
  await focusBody(app);
  await closeComposer(app);
  kept++;
  await expectLeft("c, subject only", null);

  // A resumed draft, closed untouched, is kept.
  await open(app, "Person 1");
  await app.reply("r");
  await expect(app.editor).toHaveText("Typed body");
  await closeComposer(app);
  await expectLeft("resumed, untouched", 1);

  // A draft reopened by undo send, closed untouched, is kept (DEVIATIONS).
  await open(app, "Person 4");
  await app.reply("r");
  await app.type("Undo me");
  await sendSaved(app);
  await app.toast("Sending…").getByRole("button", { name: "Undo" }).click();
  await expect(app.editor).toHaveText("Undo me");
  await app.settle();
  await closeComposer(app);
  kept++;
  await expectLeft("reopened by undo send, untouched", 1);
});

test("NEW-08 Cmd+K in compose", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  await app.compose();
  await fill(app, "kim@kite.test", "Link test", "Click here");

  // Focused, with "here" selected: the link input.
  await app.press(..."here".split("").map(() => "Shift+ArrowLeft"));
  await app.press("Meta+k");
  const input = app.page.getByPlaceholder("https://");
  await expect(input).toBeFocused();
  await app.type("example.org");
  await app.press("Enter");
  await expect(app.editor.locator('a[href*="example.org"]')).toHaveText("here");
  await expect(app.page.getByRole("dialog")).toHaveCount(0);

  // Blurred: the draft-actions palette; Discard draft removes it here and in Gmail.
  await app.settle();
  expect(await gmailDrafts(gmail, A, "Link test")).toHaveLength(1);
  await app.press("Escape");
  await expect(app.editor).not.toBeFocused();
  await app.press("Meta+k");
  const actions = app.page.getByRole("dialog", { name: "Draft actions" });
  await expect(actions).toBeVisible();
  await actions.getByRole("option", { name: /Discard draft/ }).click();
  await expect(newMessage(app)).toHaveCount(0);
  await app.settle();
  expect(await gmailDrafts(gmail, A)).toHaveLength(0);
  expect(await draftRows(app)).toEqual([]);
});

test("NEW-09 sending with no recipients never loses the draft; text still in To is sent", async ({
  app,
  gmail,
}) => {
  test.fail(
    true,
    "BUG: an address typed in To but not yet a chip is dropped on Cmd+Enter when the draft has no unsaved edit: openComposer.pending() returns null when not dirty (src/compose/Composer.tsx:258), so commitField never runs: with no chips the send is refused with 'Add a recipient', otherwise it goes out without the typed address (src/compose/start.ts:244-254)",
  );
  await seeds.standard(gmail, app);

  // Empty To, no chips: refused with a toast (Q26); the draft stays.
  await app.compose();
  await focusSubject(app);
  await app.type("Nobody yet");
  await focusBody(app);
  await app.type("Hello");
  await app.settle();
  await app.press("Meta+Enter");
  // The editor's and the window's Cmd+Enter handlers both refuse, so the toast can show twice.
  await expect(app.toast("Add a recipient").first()).toBeVisible();
  await expect(newMessage(app)).toBeVisible();
  await expect(app.toast("Sending…")).toHaveCount(0);
  await app.advance(10_000);
  await app.settle();
  expect(await gmail.requests("drafts.send")).toEqual([]);
  expect(await gmailDrafts(gmail, A, "Nobody yet")).toHaveLength(1);

  // "pat@p.test" typed but not yet a chip: sent to it.
  await app.press("Meta+Shift+o");
  await app.type("pat@p.test");
  await expect(app.chips("To")).toHaveCount(0);
  await app.press("Meta+Enter");
  await expect(newMessage(app)).toHaveCount(0);
  await app.advance(10_000);
  await app.settle();
  const m = await sentWithSubject(gmail, A, "Nobody yet");
  expect(emails(m.headers.to)).toContain("pat@p.test");
});

test("NEW-10 From picker and the sending account", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);

  await app.compose();
  await expect(fromButton(app)).toContainText(A);
  await expect(fromButton(app)).not.toContainText(A_ALIAS);
  await app.press("Meta+Shift+f");
  await app.page
    .getByRole("dialog", { name: "Send from" })
    .getByRole("option", { name: new RegExp(A_ALIAS.replace(/\./g, "\\.")) })
    .click();
  await expect(fromButton(app)).toContainText(A_ALIAS);
  await fill(app, "kim@kite.test", "From the alias", "Hi");
  await sendSaved(app);

  await app.press("Control+2");
  await expect(app.row("B 2")).toHaveCount(1);
  await app.compose();
  // B has one sendAs address: no From row.
  await expect(app.page.getByText("From", { exact: true })).toHaveCount(0);
  await fill(app, "kim@kite.test", "From Ben", "Hi");
  await sendSaved(app);

  await app.advance(10_000);
  await app.settle();
  const fromA = await sentWithSubject(gmail, A, "From the alias");
  expect(emails(fromA.headers.from)).toEqual([A_ALIAS]);
  const fromB = await sentWithSubject(gmail, B, "From Ben");
  expect(emails(fromB.headers.from)).toEqual([B]);
  expect((await sentByApp(gmail, A)).map((m) => m.headers.subject)).not.toContain("From Ben");
});

test("NEW-11 Cmd+Shift+, discards locally and in Gmail", async ({ app, gmail }) => {
  const s = await seeds.drafts(gmail, app);
  const before = (await gmailDrafts(gmail, A)).length;

  // A new message.
  await app.compose();
  await fill(app, "kim@kite.test", "Discard me", "Soon gone");
  await app.settle();
  const [made] = await gmailDrafts(gmail, A, "Discard me");
  expect(made).toBeDefined();
  await app.press("Meta+Shift+Comma");
  await expect(newMessage(app)).toHaveCount(0);
  await expect(app.toast("Draft discarded")).toBeVisible();
  await app.settle();
  expect((await gmail.requests("drafts.delete")).map((r) => r.path)).toEqual([
    expect.stringContaining(made?.id ?? "missing"),
  ]);
  expect(await gmailDrafts(gmail, A)).toHaveLength(before);
  expect(await draftRows(app)).not.toContain("Discard me");

  // The resumed Gmail draft dNew.
  const dNew = s.draft("dNew");
  await rail(app, "Drafts (g d)").click();
  await app.row("Idea").click();
  await expect(newMessage(app)).toBeVisible();
  await expect(app.editor).toHaveText("An idea for later.");
  await app.press("Meta+Shift+Comma");
  await expect(newMessage(app)).toHaveCount(0);
  await expect(app.toast("Draft discarded")).toHaveCount(2);
  await app.settle();
  expect((await gmail.requests("drafts.delete")).map((r) => r.path)).toContainEqual(
    expect.stringContaining(dNew.id),
  );
  expect((await gmailDrafts(gmail, A)).map((d) => d.id)).not.toContain(dNew.id);
  await expect(app.row("Idea")).toHaveCount(0);
});

test("NEW-12 opening another composer first closes the current one, as a second Esc would", async ({
  app,
  gmail,
}) => {
  test.fail(
    true,
    "BUG: a rail click does not close the open composer as a second Esc would: goTo (src/commands.ts:221-226) only sets `inline: null` / the list screen, with no closeComposer action, so an untouched fresh draft is never discarded and a touched one's pending save rides on a timer that the next composer's openComposer.pending (src/compose/Composer.tsx:175-178) can swallow",
  );
  test.slow();
  const s = await seeds.drafts(gmail, app);
  const drafts = async () => gmailDrafts(gmail, A);
  const start = (await drafts()).length;

  // Reply, touched: opening another draft (dEarly's stub) keeps it. SPEC Keyboard
  // makes `c` do nothing while a composer is open, so it cannot open one here.
  await open(app, "Q4 plan");
  await app.reply("r");
  await app.type("touched one");
  await app.press("Escape", "c");
  await app.settle();
  await expect(app.editor).toHaveText("touched one");
  await expect(newMessage(app)).toHaveCount(0);
  await stubs(app).filter({ hasText: "About message three." }).click();
  await expect(app.editor).toHaveText("About message three.");
  await app.settle();
  await expect(stubs(app).filter({ hasText: "touched one" })).toHaveCount(1);
  expect(await drafts()).toHaveLength(start + 1);
  await closeComposer(app);
  await app.press("Escape");

  // Reply, untouched: opening dFwd's stub discards it.
  await open(app, "Contract");
  await app.reply("r");
  await stubs(app).filter({ hasText: "FYI" }).click();
  await expect(app.editor).toHaveText("FYI");
  await app.settle();
  await expect(stubs(app)).toHaveCount(0);
  expect(await drafts()).toHaveLength(start + 1);
  const fwdThread = s.thread("fwd").threadId;
  expect((await drafts()).filter((d) => d.message.threadId === fwdThread)).toHaveLength(1);
  await closeComposer(app);
  await app.press("Escape");

  // c, touched: clicking the Inbox rail icon keeps it.
  await app.compose();
  await focusSubject(app);
  await app.type("Rail kept");
  await rail(app, "Inbox").click();
  await expect(newMessage(app)).toHaveCount(0);
  await expect(app.section("Important")).toBeVisible();
  await app.settle();
  expect(await gmailDrafts(gmail, A, "Rail kept")).toHaveLength(1);
  expect(await draftRows(app)).toContain("Rail kept");

  // c, untouched: the rail click closes it as a second Esc would, so it is
  // discarded. Not a COVERAGE row; the same rule as the untouched reply above.
  const rowsBefore = await draftRows(app);
  await app.compose();
  await rail(app, "Inbox").click();
  await expect(newMessage(app)).toHaveCount(0);
  await app.settle();
  expect(await draftRows(app)).toEqual(rowsBefore);

  // Reply, touched: going to Drafts and opening another draft there keeps it.
  await open(app, "Person 1");
  await app.reply("r");
  await app.type("row four");
  await rail(app, "Drafts (g d)").click();
  await expect(app.editor).toHaveCount(0);
  await highlight(app, "Idea");
  await app.press("Enter");
  await expect(newMessage(app)).toBeVisible();
  await expect(app.editor).toHaveText("An idea for later.");
  await app.settle();
  const p1 = s.thread("p1").threadId;
  const kept = (await drafts()).filter((d) => d.message.threadId === p1);
  expect(kept).toHaveLength(1);
  expect(htmlOf(parseMime(kept[0]?.message.raw ?? ""))).toContain("row four");
});
