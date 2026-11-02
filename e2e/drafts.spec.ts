// Drafts (COVERAGE 5.15, SPEC §Drafts, §Reading, §UI state): autosave and
// upsert, `Enter` in Drafts, Gmail-side edits and deletes, offline, reload
// and crash, and the app's own drafts read back.
import type { App } from "./app.ts";
import type { Control, GmailState } from "./control.ts";
import { expect, test } from "./fixtures.ts";
import { htmlOf, parseMime } from "./mime.ts";
import * as seeds from "./seeds.ts";
import { A } from "./seeds.ts";

type Draft = GmailState["drafts"][number];

/** A's Gmail drafts on one Gmail thread. */
async function draftsIn(gmail: Control, threadId: string): Promise<Draft[]> {
  return (await gmail.gmail(A)).drafts.filter((d) => d.message.threadId === threadId);
}

/** The HTML body of a draft. */
const bodyOf = (d: Draft | undefined) => htmlOf(parseMime(d?.message.raw ?? ""));

/** Opens a conversation from the list by clicking its row. */
async function openThread(app: App, subject: string): Promise<void> {
  await app.row(subject).first().click();
  await expect(app.conversationTitle).toContainText(subject);
}

/** Moves the highlight with `j` / `k` until it is on the row containing `text`. */
async function highlight(app: App, text: string | RegExp, left = 40): Promise<void> {
  if ((await app.highlighted.filter({ hasText: text }).count()) > 0) return;
  if (left === 0) throw new Error(`no row ${String(text)} reached with j and k`);
  const keys = await app.rows.evaluateAll((els) => els.map((el) => el.getAttribute("data-row")));
  const target = keys.indexOf(await app.row(text).first().getAttribute("data-row"));
  const lit = (await app.highlighted.count()) > 0;
  const at = lit ? keys.indexOf(await app.highlighted.getAttribute("data-row")) : -1;
  await app.press(at < target ? "j" : "k");
  await highlight(app, text, left - 1);
}

/** Opens the Drafts view. */
async function goDrafts(app: App): Promise<void> {
  await app.press("g", "d");
  await app.settle();
}

/** A draft stub in the open conversation. */
const stub = (app: App, text: string) =>
  app.conversation.getByRole("button", { name: /Draft/ }).filter({ hasText: text });

const updates = (gmail: Control) => gmail.requests("drafts.update", { account: A });
const creates = (gmail: Control) => gmail.requests("drafts.create", { account: A });

test("DRF-01 drafts autosave and update a single Gmail draft", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  const thread = s.thread("p3").threadId;
  await openThread(app, "Person 3");
  await app.reply();
  await app.type("one");
  await app.settle();

  const first = await draftsIn(gmail, thread);
  expect(first).toHaveLength(1);
  const gmailId = first[0]?.id;
  expect(bodyOf(first[0])).toContain("one");
  const local = first[0]?.message.headers["x-ubermensch-draft"];
  expect(local).toMatch(/\S/);

  const updatesBefore = (await updates(gmail)).length;
  await app.type(" two");
  await app.settle();
  const second = await draftsIn(gmail, thread);
  expect(second.map((d) => d.id)).toEqual([gmailId]);
  expect(bodyOf(second[0])).toContain("one two");
  expect((await updates(gmail)).length).toBeGreaterThan(updatesBefore);
  expect(await creates(gmail)).toHaveLength(1);

  await app.press("Escape", "Escape");
  await goDrafts(app);
  await expect(app.rows).toHaveCount(1);
  await expect(app.rows.first()).toContainText("one two");
  // The Drafts list is keyed by the local draft id, the one the header carries.
  expect(await app.rows.first().getAttribute("data-row")).toBe(local);
});

test("DRF-02 Enter in Drafts opens a draft according to its kind", async ({ app, gmail }) => {
  await seeds.drafts(gmail, app);
  await goDrafts(app);
  const count = await app.rows.count();

  // A new-message draft opens the `c` screen; Esc Esc returns to Drafts.
  await highlight(app, "Idea");
  await app.press("Enter");
  await expect(app.page.getByRole("heading", { name: "New Message" })).toBeVisible();
  await expect(app.subject).toHaveValue("Idea");
  await expect(app.editor).toContainText("An idea for later.");
  await expect(app.chips("To")).toHaveAttribute("data-chip", "kim@kite.test");
  await app.press("Escape", "Escape");
  await expect(app.page.getByRole("heading", { name: "New Message" })).toHaveCount(0);
  await expect(app.rows).toHaveCount(count);
  await expect(app.highlighted).toContainText("Idea");

  // A reply draft opens its thread with the inline composer; its target is selected.
  await highlight(app, "Person 4");
  await app.press("Enter");
  await expect(app.conversationTitle).toContainText("Person 4");
  await expect(app.editor).toContainText("My text");
  await expect(app.selectedMessage).toContainText("Sol Snow");
  await app.press("Escape", "Escape");
  await goDrafts(app);

  // A forward draft opens its thread with the forward composer.
  await highlight(app, "Fwd: Contract");
  await app.press("Enter");
  await expect(app.conversationTitle).toContainText("Contract");
  await expect(app.editor).toContainText("FYI");
});

test("DRF-03 edits made to a draft in Gmail, with the draft closed or open", async ({
  app,
  gmail,
}) => {
  test.fail(
    true,
    "BUG: a Gmail change skipped while a composer had the draft open is never read on close: src/drafts.rs:486 gates the read on is_untouched(), which is false for every draft without defaults_json (all Gmail-made drafts)",
  );
  const s = await seeds.drafts(gmail, app);
  /** The draft edited in Gmail web: its message is replaced. */
  const webEdit = (key: string, thread: string, text: string) =>
    gmail.updateDraft(
      A,
      s.draft(key).id,
      {
        subject: `Re: ${s.thread(thread).subject}`,
        threadId: s.thread(thread).threadId,
        inReplyTo: s.latest(thread).messageId,
        html: `<p>${text}</p>`,
      },
      { poll: true },
    );

  // Closed: resuming shows Gmail's version.
  await webEdit("dLatest", "p4", "from web");
  await app.settle();
  await openThread(app, "Person 4");
  await app.reply("r");
  await expect(app.editor).toContainText("from web");

  // Open, and the user types: the editor keeps the user's text, and the
  // app's next upsert overwrites Gmail's version (Q24).
  await webEdit("dLatest", "p4", "from web 3");
  await app.settle();
  await expect(app.editor).not.toContainText("from web 3");
  await app.press("Meta+Shift+m");
  await app.type(" mine");
  await app.settle();
  await expect(app.editor).toContainText("from web mine");
  await app.press("Escape", "Escape");
  await app.settle();
  const [p4Draft] = await draftsIn(gmail, s.thread("p4").threadId);
  expect(bodyOf(p4Draft)).toContain("mine");
  expect(bodyOf(p4Draft)).not.toContain("from web 3");
  await app.press("Escape");

  // Open, no edits: not overwritten while open; read once it closes.
  await openThread(app, "Person 2");
  await app.reply("r");
  await expect(app.editor).toContainText("dTwoB");
  await webEdit("dTwoB", "p2", "from web 2");
  await app.settle();
  await expect(app.editor).toContainText("dTwoB");
  await expect(app.editor).not.toContainText("from web 2");
  await app.press("Escape", "Escape");
  await app.settle();
  await app.reply("r");
  await expect(app.editor).toContainText("from web 2");
});

test("DRF-04 a draft deleted or sent elsewhere", async ({ app, gmail }) => {
  test.fail(
    true,
    "BUG: an open Gmail draft with attachments, deleted elsewhere, never reaches Gmail again: the composer saves its first-load attachments (path null), so the upsert needs the deleted source attachment row and is rejected ('attachment N is gone', src/engine/attachments.rs:70 via src/engine/drafts.rs:113)",
  );
  const s = await seeds.drafts(gmail, app);
  const gone = app.toast("Draft was sent or deleted elsewhere");

  /** Composer closed: the stub and the Drafts row disappear. */
  const closed = async (key: string, thread: string, text: string, how: "delete" | "send") => {
    await goDrafts(app);
    await expect(app.row(text)).toHaveCount(1);
    if (how === "delete") await gmail.deleteDraft(A, s.draft(key).id, { poll: true });
    else await gmail.sendDraft(A, s.draft(key).id, { poll: true });
    await app.settle();
    await expect(app.row(text)).toHaveCount(0);
    await app.press("g", "i");
    await openThread(app, thread);
    await app.settle();
    await expect(stub(app, text)).toHaveCount(0);
    await app.press("Escape");
  };

  /**
   * Composer open: it stays with its text, a toast says so, and the next
   * keystroke makes a new Gmail draft with `drafts.create`.
   */
  const open = async (key: string, thread: string, text: string, how: "delete" | "send") => {
    const gmailThread = s.thread(thread).threadId;
    await app.press("g", "i");
    await openThread(app, s.thread(thread).subject);
    await app.reply("r");
    await expect(app.editor).toContainText(text);
    const createsBefore = (await creates(gmail)).length;
    if (how === "delete") await gmail.deleteDraft(A, s.draft(key).id, { poll: true });
    else await gmail.sendDraft(A, s.draft(key).id, { poll: true });
    await app.settle();
    await expect(gone).toBeVisible();
    await expect(app.editor).toContainText(text);
    await app.press("Meta+Shift+m");
    await app.type(` ${how} again`);
    await app.settle();
    expect(await creates(gmail)).toHaveLength(createsBefore + 1);
    const now = await draftsIn(gmail, gmailThread);
    expect(now).toHaveLength(1);
    expect(now[0]?.id).not.toBe(s.draft(key).id);
    expect(bodyOf(now[0])).toContain(`${how} again`);
    await app.press("Escape", "Escape", "Escape");
    // The next toast must be a new one.
    await expect(gone).toHaveCount(0, { timeout: 10_000 });
  };

  await closed("dEarly", "Q4 plan", "About message three.", "delete");
  await open("dLatest", "p4", "My text", "delete");
  await closed("dTwoA", "Person 2", "dTwoA", "send");
  await open("dUnknown", "p6", "Aimed at nobody.", "send");
});

test("DRF-05 discarding while offline is queued and replayed in order", async ({ app, gmail }) => {
  const s = await seeds.drafts(gmail, app);
  const p4 = s.thread("p4").threadId;
  await gmail.offline(true);
  await openThread(app, "Person 4");
  await app.reply("r");
  await expect(app.editor).toContainText("My text");
  await app.press("Meta+Shift+m");
  await app.type(" and more");
  // The upsert is queued while offline.
  await app.settle();
  await app.press("Meta+Shift+Comma");

  // Gone locally at once.
  await expect(app.editor).toHaveCount(0);
  await expect(stub(app, "My text")).toHaveCount(0);
  await app.press("Escape");
  await goDrafts(app);
  await expect(app.row("Person 4")).toHaveCount(0);

  await gmail.offline(false);
  await app.settle();
  const drafts = (await gmail.gmail(A)).drafts;
  expect(drafts.map((d) => d.id)).not.toContain(s.draft("dLatest").id);
  expect(drafts.filter((d) => d.message.threadId === p4)).toEqual([]);
  await expect(app.row("Person 4")).toHaveCount(0);
});

test("DRF-06 a draft written offline reaches Gmail exactly once", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  await gmail.offline(true);
  await app.compose();
  await app.type("kim@kite.test,");
  await app.subject.fill("Offline idea");
  await app.editor.click();
  await app.type("first body");
  await app.settle();
  await app.press("Escape", "Escape");

  await goDrafts(app);
  await expect(app.row("Offline idea")).toHaveCount(1);

  await highlight(app, "Offline idea");
  await app.press("Enter");
  await expect(app.editor).toContainText("first body");
  await app.editor.click();
  await app.press("Meta+a");
  await app.type("final body");
  await app.settle();
  await app.press("Escape", "Escape");

  await gmail.offline(false);
  await app.settle();
  const mine = (await gmail.gmail(A)).drafts.filter(
    (d) => d.message.headers.subject === "Offline idea",
  );
  expect(mine).toHaveLength(1);
  expect(bodyOf(mine[0])).toContain("final body");
  expect(bodyOf(mine[0])).not.toContain("first body");
});

test("DRF-07 sending cancels a queued upsert", async ({ app, gmail }) => {
  const s = await seeds.drafts(gmail, app);
  const p4 = s.thread("p4").threadId;
  await gmail.offline(true);
  await openThread(app, "Person 4");
  await app.reply("r");
  await expect(app.editor).toContainText("My text");
  await app.press("Meta+Shift+m");
  await app.type(" final");
  await app.settle();
  await app.press("Meta+Enter");
  await gmail.offline(false);
  await app.advance(10_000);
  await app.settle();

  const sent = (await gmail.gmail(A)).messages.filter(
    (m) => m.threadId === p4 && m.labels.includes("SENT"),
  );
  expect(sent).toHaveLength(1);
  expect(htmlOf(parseMime(sent[0]?.raw ?? ""))).toContain("final");

  const log = await gmail.requests(undefined, { account: A });
  const send = log.find((r) => r.api === "drafts.send");
  expect(send).toBeDefined();
  const late = log.filter((r) => r.api === "drafts.update" && r.seq > (send?.seq ?? 0));
  expect(late).toEqual([]);
  expect(await draftsIn(gmail, p4)).toEqual([]);
});

test("DRF-08 a webview reload or a crash with a composer open", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);

  // `r`, touched, then a reload: kept, as a stub and as a Gmail draft.
  await openThread(app, "Person 3");
  await app.reply();
  await app.type("keep after reload");
  await app.settle();
  await app.reload();
  await app.settle();
  expect(await draftsIn(gmail, s.thread("p3").threadId)).toHaveLength(1);
  await app.press("g", "i");
  await openThread(app, "Person 3");
  await expect(stub(app, "keep after reload")).toHaveCount(1);
  await app.press("Escape");

  // After the reload, actions still reach Gmail.
  await app.press("g", "i");
  await highlight(app, "Person 1");
  await app.press("e");
  await app.settle();
  const p1 = (await gmail.gmail(A)).messages.find((m) => m.id === s.message("p1", 1).id);
  expect(p1?.labels).not.toContain("INBOX");

  // `r`, untouched, then a reload: discarded.
  await openThread(app, "Person 5");
  await app.reply();
  await app.reload();
  await app.settle();
  expect(await draftsIn(gmail, s.thread("p5").threadId)).toEqual([]);
  await goDrafts(app);
  await expect(app.row("Person 5")).toHaveCount(0);

  // `c`, touched, then a crash: kept.
  await app.compose();
  await app.subject.fill("Crash kept");
  await app.settle();
  await app.crashAndRelaunch();
  await goDrafts(app);
  await expect(app.row("Crash kept")).toHaveCount(1);

  // `c`, untouched, then a crash: discarded on launch.
  await app.compose();
  await app.crashAndRelaunch();
  await goDrafts(app);
  await expect(app.rows).toHaveCount(2);
  await expect(app.row("Crash kept")).toHaveCount(1);
  await expect(app.row("keep after reload")).toHaveCount(1);
  expect((await gmail.gmail(A)).drafts).toHaveLength(2);
});

test("DRF-09 the app's own drafts are read back after the account is re-added", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  await openThread(app, "Offsite venue");
  await app.reply();
  await app.type("keep me");
  await app.settle();
  await app.press("Escape", "Escape", "Escape");
  await app.settle();

  // Remove A, then add it again.
  await app.press("Meta+,");
  const settings = app.sheet("Settings");
  await settings.getByRole("button", { name: "Remove account" }).first().click();
  await settings.getByRole("button", { name: "Remove", exact: true }).click();
  await app.settle();
  await expect(app.account(A)).toHaveCount(0);
  await app.press("Escape");
  await gmail.nextSignIn({ email: A });
  await app.press("Meta+k");
  await app.type("Add account");
  await app.press("Enter");
  await expect(app.account(A)).toBeVisible();
  await app.settle();
  await app.account(A).click();
  await app.settle();

  await goDrafts(app);
  await expect(app.rows).toHaveCount(1);
  await expect(app.rows.first()).toContainText("Offsite venue");
  await expect(app.rows.first()).toContainText("keep me");
  await app.press("Enter");
  await expect(app.conversationTitle).toContainText("Offsite venue");
  await expect(app.editor).toContainText("keep me");
  // The quote is its own block, not part of the editor.
  await expect(app.editor).not.toContainText("Which venue");
  await expect(app.page.getByTitle("Show quoted text")).toBeVisible();
  expect((await gmail.gmail(A)).drafts).toHaveLength(1);
});
