// Drafts (SPEC §Drafts, §Reading, §UI state): autosave and
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

test("DRF-12 an edit saved while the draft's upsert is in flight still reaches Gmail", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  const thread = s.thread("p3").threadId;
  await openThread(app, "Person 3");
  await app.reply();
  await app.type("one");
  await app.settle();

  const write = await gmail.hold("drafts.update");
  await app.type(" two");
  await expect.poll(async () => (await updates(gmail)).some((r) => r.held)).toBe(true);
  // Saved while the write of "one two" is in flight.
  await app.type(" three");
  await app.settle();
  await write.release();
  await app.settle();

  const now = await draftsIn(gmail, thread);
  expect(now).toHaveLength(1);
  expect(bodyOf(now[0])).toContain("one two three");
});

test("DRF-13 a draft discarded while its first upsert is in flight stays gone", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  const thread = s.thread("p3").threadId;
  const create = await gmail.hold("drafts.create");
  await openThread(app, "Person 3");
  await app.reply();
  await app.type("discard me");
  await expect.poll(async () => (await creates(gmail)).some((r) => r.held)).toBe(true);
  await app.press("Meta+Shift+Comma");
  await expect(app.editor).toHaveCount(0);
  await app.settle();

  // The create lands after the discard. A poll reconciles drafts before the
  // copy it made is deleted, and again after.
  const del = await gmail.hold("drafts.delete");
  await create.release();
  await app.settle();
  await gmail.poll();
  await app.settle();
  await expect(stub(app, "discard me")).toHaveCount(0);
  await del.release();
  await app.settle();
  await gmail.poll();
  await app.settle();
  expect(await draftsIn(gmail, thread)).toEqual([]);
  await expect(stub(app, "discard me")).toHaveCount(0);
  await app.press("Escape");
  await goDrafts(app);
  await expect(app.row("Person 3")).toHaveCount(0);
});

test("DRF-14 a second Gmail draft carrying a local draft's id is deleted as an extra copy", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  const thread = s.thread("p3").threadId;
  await openThread(app, "Person 3");
  await app.reply();
  await app.type("the real one");
  await app.settle();
  await app.press("Escape", "Escape");
  await app.settle();
  const [mine] = await draftsIn(gmail, thread);
  const local = mine?.message.headers["x-ubermensch-draft"] ?? "";
  expect(local).toMatch(/\S/);

  // Another Gmail draft turns up with the same local id, for example from a
  // create whose answer was lost.
  await gmail.createDraft(
    A,
    {
      subject: "Re: Person 3",
      threadId: thread,
      inReplyTo: s.latest("p3").messageId,
      html: "<p>a stray copy</p>",
      headers: [["X-Ubermensch-Draft", local]],
    },
    { poll: true },
  );
  await app.settle();

  const now = await draftsIn(gmail, thread);
  expect(now.map((d) => d.id)).toEqual([mine?.id]);
  expect(bodyOf(now[0])).toContain("the real one");
  await expect(stub(app, "the real one")).toHaveCount(1);
  await expect(stub(app, "a stray copy")).toHaveCount(0);
});

test("DRF-15 a draft reconcile on a stale list keeps a draft written meanwhile", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  // Another client's draft makes the next poll reconcile drafts; the list's
  // answer is taken now and arrives late.
  await gmail.createDraft(A, {
    subject: "From the web",
    to: ["kim@kite.test"],
    html: "<p>web</p>",
  });
  const list = await gmail.holdAnswer("drafts.list");
  await gmail.poll();
  await app.settle();

  // Meanwhile a reply is written to Gmail and closed.
  await openThread(app, "Person 4");
  await app.reply();
  await app.type("written meanwhile");
  await app.settle();
  await app.press("Escape", "Escape");
  await app.settle();
  expect(await draftsIn(gmail, s.thread("p4").threadId)).toHaveLength(1);

  await list.release();
  await app.settle();
  await expect(stub(app, "written meanwhile")).toHaveCount(1);
  await app.press("Escape");
  await goDrafts(app);
  await expect(app.row("written meanwhile")).toHaveCount(1);
  await expect(app.row("From the web")).toHaveCount(1);
});

test("DRF-16 a Gmail edit read back after the draft was reopened and edited does not replace the edit", async ({
  app,
  gmail,
}) => {
  const s = await seeds.drafts(gmail, app);
  const p6 = s.thread("p6");
  // Edited in Gmail web while closed; reading that edit back is slow.
  await gmail.updateDraft(A, s.draft("dUnknown").id, {
    subject: `Re: ${p6.subject}`,
    threadId: p6.threadId,
    inReplyTo: "<nobody@x.test>",
    html: "<p>from web</p>",
  });
  const read = await gmail.holdAnswer({ api: "drafts.get", query: { format: "full" } });
  await gmail.poll();
  await app.settle();

  // Meanwhile the draft is reopened and edited.
  await openThread(app, "Person 6");
  await app.reply("r");
  await expect(app.editor).toContainText("Aimed at nobody.");
  await app.press("Meta+Shift+m");
  await app.type(" Mine.");
  await app.settle();

  await read.release();
  await app.settle();
  await app.press("Escape", "Escape");
  await app.settle();
  // The app's edit wins, as the last writer (SPEC Drafts): locally and in Gmail.
  await app.reply("r");
  await expect(app.editor).toContainText("Aimed at nobody. Mine.");
  await expect(app.editor).not.toContainText("from web");
  const [d] = await draftsIn(gmail, p6.threadId);
  expect(bodyOf(d)).toContain("Aimed at nobody. Mine.");
});

test("DRF-17 a Gmail draft read before its thread's messages is aimed at the thread's messages", async ({
  app,
  gmail,
}) => {
  const s = await seeds.drafts(gmail, app, { open: false });
  // The connect sync reads p2's drafts while the fetch of p2's messages is slow.
  const thread = await gmail.hold({
    api: "threads.get",
    path: `*/threads/${s.thread("p2").threadId}`,
  });
  await app.open();
  await thread.release();
  await app.settle();

  // `r` on the latest message resumes the newest draft started there.
  await openThread(app, "Person 2");
  await app.reply("r");
  await expect(app.editor).toContainText("dTwoB");
});

test("DRF-18 a crash after drafts.create reached Gmail, before its ids were stored, leaves one draft", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  const thread = s.thread("p3").threadId;
  // Gmail makes the draft, and its answer is still on the way when the app dies.
  const create = await gmail.holdAnswer("drafts.create");
  await openThread(app, "Person 3");
  await app.reply();
  await app.type("before the crash");
  await expect.poll(async () => (await creates(gmail)).some((r) => r.held)).toBe(true);
  const [made] = await draftsIn(gmail, thread);
  expect(made).toBeDefined();
  await app.crashAndRelaunch(async () => {
    // The crash dropped the parked answer; end the hold.
    await create.release(undefined, { min: 0 });
  });
  await app.settle();

  // The replayed upsert found the draft by its header instead of making another.
  expect(await creates(gmail)).toHaveLength(1);
  const now = await draftsIn(gmail, thread);
  expect(now.map((d) => d.id)).toEqual([made?.id]);
  expect(bodyOf(now[0])).toContain("before the crash");
  await goDrafts(app);
  await expect(app.rows).toHaveCount(1);
  await expect(app.row("before the crash")).toHaveCount(1);
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

  // A reply aimed at an older message of an unread thread: that message is
  // selected, not the latest one the conversation would select on its own.
  await highlight(app, "About message three.");
  await app.press("Enter");
  await expect(app.conversationTitle).toContainText("Q4 plan");
  await expect(app.editor).toContainText("About message three.");
  await expect(app.selectedMessage).toContainText("Plan message 3");
  await app.press("Escape", "Escape");
  await goDrafts(app);

  // A forward draft opens its thread in the same reply composer (SPEC Reading).
  await highlight(app, "Fwd: Contract");
  await app.press("Enter");
  await expect(app.conversationTitle).toContainText("Contract");
  await expect(app.editor).toContainText("FYI");
});

test("DRF-03 edits made to a draft in Gmail, with the draft closed or open", async ({
  app,
  gmail,
}) => {
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
  // app's next upsert overwrites Gmail's version (SPEC Drafts: the last writer wins).
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
  await app.press("Escape", "Escape", "Escape");

  // Open, no edits, then sent: the send carries the text on screen, so the
  // local draft was not replaced with Gmail's version behind the editor.
  await openThread(app, "Person 6");
  await app.reply("r");
  await expect(app.editor).toContainText("Aimed at nobody.");
  await webEdit("dUnknown", "p6", "from web 6");
  await app.settle();
  await app.press("Meta+Enter");
  await app.advance(10_000);
  await app.settle();
  const sent = (await gmail.gmail(A)).messages.find(
    (m) => m.threadId === s.thread("p6").threadId && m.labels.includes("SENT"),
  );
  expect(htmlOf(parseMime(sent?.raw ?? ""))).toContain("Aimed at nobody.");
  expect(htmlOf(parseMime(sent?.raw ?? ""))).not.toContain("from web 6");
});

test("DRF-04 a draft deleted or sent elsewhere", async ({ app, gmail }) => {
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
    const seqBefore = Math.max(0, ...(await gmail.requests()).map((r) => r.seq));
    await app.press("Meta+Shift+m");
    await app.type(` ${how} again`);
    await app.settle();
    const next = (await gmail.requests(undefined, { account: A })).find(
      (r) => r.seq > seqBefore && (r.api === "drafts.create" || r.api === "drafts.update"),
    );
    expect(next?.api, "the next upsert after the draft left Gmail").toBe("drafts.create");
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

  // Requests made offline never reach the fake, so the log holds only what ran
  // after reconnecting: the outbox's own write, then the send. A queued upsert
  // that survived the send would add a second `drafts.update`.
  const log = await gmail.requests(undefined, { account: A });
  const writes = log.filter((r) => r.api === "drafts.update" || r.api === "drafts.send");
  expect(writes.map((r) => r.api)).toEqual(["drafts.update", "drafts.send"]);
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

/** The open composer's body as Rust last sent it (the mirror's copy). A string: `__ub` is the app's hook. */
const MIRROR_BODY = `Object.values(window.__ub.useMirror.getState().subs)
  .find((s) => s.type === "composer")?.draft?.bodyHtml ?? null`;

test("DRF-10 text deleted while its save is on the way stays deleted", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  await openThread(app, "Person 3");
  await app.reply();
  await app.type("Hello");
  const release = await app.park("submit");
  // The autosave fires; its submit is parked.
  await app.page.clock.runFor(500);
  await app.press("Meta+a", "Backspace");
  await expect(app.editor).toHaveText("");
  await release();
  // The parked save's echo arrives with "Hello".
  await expect.poll(() => app.page.evaluate<string | null>(MIRROR_BODY)).toContain("Hello");
  await expect(app.editor).toHaveText("");

  // An empty draft is not upserted, so a new word shows what Gmail keeps.
  await app.type("Bye");
  await app.page.clock.runFor(500);
  await app.settle();
  await expect(app.editor).toHaveText("Bye");
  const [draft] = await draftsIn(gmail, s.thread("p3").threadId);
  expect(bodyOf(draft)).toContain("Bye");
  expect(bodyOf(draft)).not.toContain("Hello");
});

test("DRF-11 typing before a reply's first load keeps its filled fields", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  await openThread(app, "Offsite venue");
  // The composer's subscribe is parked, so its first load waits.
  const release = await app.park("submit", (body) => body.includes('"type":"composer"'));
  await app.press("r");
  await expect(app.editor).toBeFocused();
  await app.type("Early words");
  await app.page.clock.runFor(500);
  await release();
  await expect(app.chips("To")).toHaveCount(1);
  await app.page.clock.runFor(500);
  await app.settle();

  await expect(app.chips("To")).toHaveAttribute("data-chip", "carol@corp.test");
  await expect(app.editor).toHaveText("Early words");
  const [draft] = await draftsIn(gmail, s.thread("addr").threadId);
  expect(draft?.message.headers.to).toContain("carol@corp.test");
  expect(draft?.message.headers.subject).toBe("Re: Offsite venue");
  expect(bodyOf(draft)).toContain("Early words");

  // Resumed with its load parked, then closed at once: the text typed
  // meanwhile follows the saved body.
  await app.press("Escape", "Escape");
  await app.settle();
  const again = await app.park("submit", (body) => body.includes('"type":"composer"'));
  await app.press("r");
  await expect(app.editor).toBeFocused();
  await app.type("More words");
  await app.press("Escape", "Escape");
  await expect(app.editor).toHaveCount(0);
  await again();
  await app.settle();
  const [kept] = await draftsIn(gmail, s.thread("addr").threadId);
  expect(kept?.id).toBe(draft?.id);
  expect(kept?.message.headers.to).toContain("carol@corp.test");
  expect(bodyOf(kept)).toMatch(/Early words[\s\S]*More words/);
});

test("DRF-19 an edit still waiting to reach Gmail is not replaced by an older Gmail version", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  const p3 = s.thread("p3");
  await openThread(app, "Person 3");
  await app.reply();
  await app.type("one");
  await app.settle();
  expect(bodyOf((await draftsIn(gmail, p3.threadId))[0])).toContain("one");

  // Gmail applies the next update, but its answer is lost: the save waits to retry.
  await gmail.fault("drafts.update", { kind: "noResponse" }, { times: 1 });
  await app.type(" two");
  await app.settle();
  expect(bodyOf((await draftsIn(gmail, p3.threadId))[0])).toContain("one two");
  await app.type(" three");
  await app.press("Escape", "Escape");
  await app.settle();
  await expect(stub(app, "one two three")).toHaveCount(1);

  // A poll sees the draft's new Gmail message: the app's own lost write.
  await gmail.poll();
  await app.settle();
  await expect(stub(app, "one two three")).toHaveCount(1);
  await app.advance(120_000, { stepwise: true });
  await app.settle();
  const [draft] = await draftsIn(gmail, p3.threadId);
  expect(bodyOf(draft)).toContain("one two three");

  // Another client edits the draft while a save waits to retry after a 503.
  await gmail.fault("drafts.update", { kind: "status", status: 503 }, { times: 1 });
  await app.reply("r");
  await expect(app.editor).toHaveText("one two three");
  await app.type(" four");
  await app.press("Escape", "Escape");
  await app.settle();
  await gmail.updateDraft(
    A,
    draft?.id ?? "",
    {
      subject: `Re: ${p3.subject}`,
      threadId: p3.threadId,
      inReplyTo: s.latest("p3").messageId,
      html: "<p>from web</p>",
    },
    { poll: true },
  );
  await app.settle();
  await expect(stub(app, "one two three four")).toHaveCount(1);
  await app.advance(120_000, { stepwise: true });
  await app.settle();
  expect(bodyOf((await draftsIn(gmail, p3.threadId))[0])).toContain("one two three four");
  await app.reply("r");
  await expect(app.editor).toHaveText("one two three four");
});

test("DRF-20 j and k in a conversation opened from Drafts open the other drafts' threads", async ({
  app,
  gmail,
}) => {
  await seeds.drafts(gmail, app);
  await goDrafts(app);
  await highlight(app, "Person 4");
  await app.press("Enter");
  await expect(app.editor).toContainText("My text");
  await app.press("Escape", "Escape");
  await expect(app.editor).toHaveCount(0);
  await app.settle();

  // Drafts, latest edit first: … Q4 plan, Person 4, Contract (a forward), Idea
  // (a new message). `j` opens the next draft's thread.
  await app.press("j");
  await expect(app.conversationTitle).toContainText("Contract");
  await app.settle();
  // A new message has no thread: `j` passes it over, and no draft follows it.
  await app.press("j");
  await app.settle();
  await expect(app.conversationTitle).toContainText("Contract");
  await app.press("k");
  await expect(app.conversationTitle).toContainText("Person 4");
  await app.press("k");
  await expect(app.conversationTitle).toContainText("Q4 plan");
  await app.settle();

  // Back in Drafts, the highlight is on the draft whose thread was open.
  await app.press("Escape");
  await expect(app.highlighted).toContainText("About message three.");
  await app.press("Enter");
  await expect(app.editor).toContainText("About message three.");
});
