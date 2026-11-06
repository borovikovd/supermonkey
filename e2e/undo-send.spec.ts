// Send and undo (COVERAGE 5.16, SPEC §Send and undo, §Undo): the outbox
// state machine, held undo, rejection, no clear answer, and restart.
import type { Locator, Route } from "@playwright/test";

import type { App } from "./app.ts";
import type { ApiMethod, Control, GmailMessage, Outcome } from "./control.ts";
import { expect, test } from "./fixtures.ts";
import { htmlOf, parseMime } from "./mime.ts";
import * as seeds from "./seeds.ts";
import { A } from "./seeds.ts";

/** The send delay (SPEC: a fixed ten seconds). */
const DELAY = 10_000;

/** A's messages on a Gmail thread that carry SENT. */
async function sentIn(gmail: Control, threadId: string): Promise<GmailMessage[]> {
  return (await gmail.gmail(A)).messages.filter(
    (m) => m.threadId === threadId && m.labels.includes("SENT"),
  );
}

const count = async (gmail: Control, api: ApiMethod) => (await gmail.requests(api)).length;

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

/**
 * From the list: opens the thread, replies with `text`, and sends. The
 * autosave runs first, so the send carries the text (a send within its delay
 * loses it: a known bug covered elsewhere).
 */
async function sendReply(app: App, subject: string, text: string): Promise<void> {
  await openThread(app, subject);
  await app.reply();
  await app.type(text);
  await app.settle();
  await app.press("Meta+Enter");
  await expect(app.editor).toHaveCount(0);
}

/** `c`, to Kim, with a subject and body, then send. */
async function sendNew(app: App, subject: string, body: string): Promise<void> {
  await app.compose();
  await app.type("kim@kite.test,");
  await app.subject.fill(subject);
  await app.editor.click();
  await app.type(body);
  await app.settle();
  await app.press("Meta+Enter");
  await expect(newMessage(app)).toHaveCount(0);
}

/** The send toast for a subject (its description). */
const sendToast = (app: App, subject: string): Locator => app.toast(subject);
const undoOn = (toast: Locator): Locator => toast.getByRole("button", { name: "Undo" });

/**
 * Moves past `send_after` without firing the page's 4 s Sent-toast timer,
 * so the toast is still open afterwards.
 */
async function passSendAfter(app: App): Promise<void> {
  await app.advance(DELAY - 1);
  await app.advance(1);
}

/** The `c` screen's heading. */
const newMessage = (app: App): Locator => app.page.getByRole("heading", { name: "New Message" });

test("SND-01 the send waits ten seconds, then goes out once", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  const p6Labels = async () =>
    (await gmail.gmail(A)).messages.find((m) => m.id === s.message("p6", 1).id)?.labels ?? [];
  // An older entry: once the send's entry leaves the history, `z` reaches it.
  await highlight(app, "Person 6");
  await app.press("e");
  await app.settle();
  expect(await p6Labels()).not.toContain("INBOX");
  await sendNew(app, "Ten seconds", "Body of the timed send");
  const toast = sendToast(app, "Ten seconds");
  await expect(toast).toContainText("Sending…");
  await expect(undoOn(toast)).toBeVisible();

  await app.advance(DELAY - 1);
  await app.settle();
  expect(await gmail.requests("drafts.send")).toEqual([]);

  await app.advance(1);
  await app.settle();
  expect(await gmail.requests("drafts.send")).toHaveLength(1);
  await expect(toast).toContainText("Sent");
  const sent = (await gmail.gmail(A)).messages.filter(
    (m) => m.labels.includes("SENT") && m.headers.subject === "Ten seconds",
  );
  expect(sent).toHaveLength(1);
  await app.press("g", "t");
  await expect(app.row("Ten seconds")).toHaveCount(1);

  // The toast closes; a stray `z` then never trashes delivered mail, and
  // undoes the entry before the send instead.
  await app.advance(5_000);
  await expect(toast).toHaveCount(0);
  await app.press("z");
  await app.settle();
  const after = (await gmail.gmail(A)).messages.find((m) => m.id === sent[0]?.id);
  expect(after?.labels).not.toContain("TRASH");
  expect(await p6Labels()).toContain("INBOX");
  expect(await gmail.requests("drafts.send")).toHaveLength(1);
});

test("SND-02 undo while waiting cancels the send and reopens the composer", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  const p3 = s.thread("p3").threadId;

  // The toast's Undo, on a reply whose thread has left the view.
  await sendReply(app, "Person 3", "undo me");
  await app.press("e");
  await app.press("g", "i");
  await expect(app.row("Person 3")).toHaveCount(0);
  await undoOn(sendToast(app, "Person 3")).click();
  await expect(app.conversationTitle).toContainText("Person 3");
  await expect(app.editor).toContainText("undo me");

  // `z`, on a new message, while the send is the newest entry.
  await app.press("Escape", "Escape", "Escape");
  await expect(app.editor).toHaveCount(0);
  await sendNew(app, "Undo by z", "z body");
  await app.press("z");
  await expect(newMessage(app)).toBeVisible();
  await expect(app.subject).toHaveValue("Undo by z");
  await expect(app.editor).toContainText("z body");

  // Nothing is ever sent, and Esc Esc keeps both drafts.
  await app.advance(60_000);
  await app.settle();
  expect(await gmail.requests("drafts.send")).toEqual([]);
  await app.press("Escape", "Escape");
  await app.settle();
  await app.press("g", "d");
  await expect(app.row("undo me")).toHaveCount(1);
  await expect(app.row("Undo by z")).toHaveCount(1);

  // Sending again works, for both.
  await highlight(app, "undo me");
  await app.press("Enter");
  await expect(app.editor).toContainText("undo me");
  await app.press("Meta+Enter");
  await expect(app.editor).toHaveCount(0);
  await app.press("g", "d");
  await highlight(app, "Undo by z");
  await app.press("Enter");
  await expect(newMessage(app)).toBeVisible();
  await app.press("Meta+Enter");
  await expect(newMessage(app)).toHaveCount(0);
  await app.advance(DELAY);
  await app.settle();
  expect(await gmail.requests("drafts.send")).toHaveLength(2);
  const p3Sent = await sentIn(gmail, p3);
  expect(p3Sent).toHaveLength(1);
  expect(htmlOf(parseMime(p3Sent[0]?.raw ?? ""))).toContain("undo me");
  const zSent = (await gmail.gmail(A)).messages.filter(
    (m) => m.labels.includes("SENT") && m.headers.subject === "Undo by z",
  );
  expect(zSent).toHaveLength(1);
});

test("SND-03 z reaches a send only while it is the newest entry; the toast's Undo always does", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  const inbox = async (key: string) =>
    (await gmail.gmail(A)).messages.find((m) => m.id === s.message(key, 1).id)?.labels ?? [];

  // `z` after a newer `e` undoes the `e`; the send stays waiting and goes out.
  await sendReply(app, "Person 3", "first send");
  await app.press("Escape");
  await highlight(app, "Person 4");
  await app.press("e");
  await expect(app.row("Person 4")).toHaveCount(0);
  await app.press("z");
  await expect(app.row("Person 4")).toHaveCount(1);
  await app.settle();
  expect(await inbox("p4")).toContain("INBOX");
  expect(await gmail.requests("drafts.send")).toEqual([]);
  await app.advance(DELAY);
  await app.settle();
  expect(await sentIn(gmail, s.thread("p3").threadId)).toHaveLength(1);

  // The toast's Undo on a send behind a newer `e` undoes the send only.
  await sendReply(app, "Person 5", "second send");
  await app.press("Escape");
  await highlight(app, "Person 6");
  await app.press("e");
  await expect(app.row("Person 6")).toHaveCount(0);
  await undoOn(sendToast(app, "Person 5")).click();
  await expect(app.conversationTitle).toContainText("Person 5");
  await expect(app.editor).toContainText("second send");
  await app.settle();
  expect(await inbox("p6")).not.toContain("INBOX");
  await app.press("Escape", "Escape", "Escape");
  await expect(app.row("Person 6")).toHaveCount(0);
  await app.press("z");
  await expect(app.row("Person 6")).toHaveCount(1);
  await app.advance(60_000);
  await app.settle();
  expect(await inbox("p6")).toContain("INBOX");
  expect(await sentIn(gmail, s.thread("p5").threadId)).toEqual([]);
});

test("SND-04 undo after the send was accepted trashes only that message", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  const addr = s.thread("addr").threadId;
  await sendReply(app, "Offsite venue", "Let's book the lake house.");
  await passSendAfter(app);
  const toast = sendToast(app, "Offsite venue");
  await expect(toast).toContainText("Sent");
  await expect(app.messages).toHaveCount(2);
  await undoOn(toast).click();
  await app.settle();

  const [sent] = await sentIn(gmail, addr);
  expect(sent?.labels).toContain("TRASH");
  const carol = (await gmail.gmail(A)).messages.find((m) => m.id === s.message("addr", 1).id);
  expect(carol?.labels).toContain("INBOX");
  expect(carol?.labels).not.toContain("TRASH");
  await expect(app.messages).toHaveCount(1);
  await app.press("Escape", "g", "i");
  await expect(app.row("Offsite venue")).toHaveCount(1);
});

test("SND-05 undo while sending is held until the send resolves", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  const p6Labels = async () =>
    (await gmail.gmail(A)).messages.find((m) => m.id === s.message("p6", 1).id)?.labels ?? [];

  // An older entry, for `z` to reach.
  await highlight(app, "Person 6");
  await app.press("e");
  await app.settle();
  expect(await p6Labels()).not.toContain("INBOX");

  // 200: the send went out, so the app trashes it; no composer opens.
  let hold = await gmail.hold("drafts.send");
  await sendReply(app, "Person 3", "held then sent");
  await app.advance(DELAY);
  const toast3 = sendToast(app, "Person 3");
  await undoOn(toast3).click();
  await expect(toast3).toBeVisible();
  await expect(undoOn(toast3)).toHaveCount(0);
  await app.press("z");
  await expect.poll(p6Labels, { message: "z undid the e on p6, not the send" }).toContain("INBOX");
  await hold.release();
  await app.settle();
  const [sent3] = await sentIn(gmail, s.thread("p3").threadId);
  expect(sent3?.labels).toContain("TRASH");
  await expect(app.editor).toHaveCount(0);

  // 400: not sent; a notice, and the draft is back in a composer on its thread.
  await app.press("Escape", "g", "i");
  hold = await gmail.hold("drafts.send");
  await sendReply(app, "Person 5", "held then rejected");
  await app.press("Escape");
  await app.advance(DELAY);
  const toast5 = sendToast(app, "Person 5");
  await undoOn(toast5).click();
  await expect(undoOn(toast5)).toHaveCount(0);
  await hold.release({ kind: "status", status: 400 });
  await app.settle();
  await expect(app.errorToasts).not.toHaveCount(0);
  await expect(app.conversationTitle).toContainText("Person 5");
  await expect(app.editor).toContainText("held then rejected");
  expect(await sentIn(gmail, s.thread("p5").threadId)).toEqual([]);

  // drop: not sent, and drafts.get finds the draft. The held undo wins (SPEC Send and undo):
  // no second drafts.send, and the draft is back in a composer.
  await app.press("Escape", "Escape", "Escape", "g", "i");
  hold = await gmail.hold("drafts.send");
  await sendReply(app, "Person 1", "held then dropped");
  await app.press("Escape");
  await app.advance(DELAY);
  const toast1 = sendToast(app, "Person 1");
  await undoOn(toast1).click();
  await expect(undoOn(toast1)).toHaveCount(0);
  const sends = await count(gmail, "drafts.send");
  const gets = await count(gmail, "drafts.get");
  await hold.release({ kind: "drop" });
  await app.advance(30_000, { stepwise: true });
  await app.settle();
  await expect(app.conversationTitle).toContainText("Person 1");
  await expect(app.editor).toContainText("held then dropped");
  expect(await count(gmail, "drafts.get")).toBeGreaterThan(gets);
  expect(await count(gmail, "drafts.send")).toBe(sends);
  expect(await sentIn(gmail, s.thread("p1").threadId)).toEqual([]);
});

test("SND-06 a rejected send brings the draft back", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  await gmail.fault("drafts.send", { kind: "status", status: 400, reason: "Invalid To header" });

  // B on screen: the app switches to A and reopens p3 with the reply composer.
  await sendReply(app, "Person 3", "rejected reply");
  await app.press("Control+2");
  await expect(app.row("B 2")).toHaveCount(1);
  await app.advance(DELAY);
  await expect(app.conversationTitle).toContainText("Person 3");
  await expect(app.editor).toContainText("rejected reply");
  // Shown during the advance, so its own 4 s may have run out: count it even on its way out.
  await expect(
    app.page
      .locator('[data-sonner-toast][data-type="error"]')
      .filter({ hasText: "Injected fault" }),
  ).not.toHaveCount(0);
  await app.press("Escape", "Escape", "Escape");

  // Typing in another composer: no takeover; Open draft swaps composers.
  await sendReply(app, "Person 5", "rejected while typing");
  await app.compose();
  await app.editor.click();
  await app.type("typing here");
  await app.advance(DELAY);
  await app.settle();
  await expect(newMessage(app)).toBeVisible();
  await expect(app.editor).toContainText("typing here");
  await expect(app.editor).toBeFocused();
  const openDraft = app.page.getByRole("button", { name: "Open draft" });
  await expect(openDraft).toBeVisible();
  await openDraft.click();
  await expect(app.conversationTitle).toContainText("Person 5");
  await expect(app.editor).toContainText("rejected while typing");
  await app.press("Escape", "Escape", "Escape", "g", "d");
  await expect(app.row("typing here")).toHaveCount(1);

  // Idle, and the draft is a new message: the `c` screen opens with it.
  await app.press("g", "i");
  await sendNew(app, "Idle new", "rejected new message");
  await app.advance(DELAY);
  await expect(newMessage(app)).toBeVisible();
  await expect(app.subject).toHaveValue("Idle new");
  await expect(app.editor).toContainText("rejected new message");
  expect(await sentIn(gmail, s.thread("p3").threadId)).toEqual([]);
});

test("SND-07 offline at send time: Email delayed, then sent once online", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);

  // Unreachable when the timer passes: Email delayed, then sent once online.
  await sendReply(app, "Person 3", "sent late");
  await gmail.offline(true);
  await app.advance(DELAY);
  const toast3 = sendToast(app, "Person 3");
  await expect(toast3).toContainText("Email delayed");
  await expect(app.conversation.getByText("Email delayed")).toBeVisible();
  await gmail.offline(false);
  await app.settle();
  await expect(toast3).toContainText("Sent");
  expect(await sentIn(gmail, s.thread("p3").threadId)).toHaveLength(1);
  await app.press("Escape");

  // Undo after the timer passed while offline: the composer reopens; nothing is sent.
  await sendReply(app, "Person 5", "never sent");
  await gmail.offline(true);
  await app.advance(DELAY);
  const toast5 = sendToast(app, "Person 5");
  await expect(toast5).toContainText("Email delayed");
  await undoOn(toast5).click();
  await expect(app.conversationTitle).toContainText("Person 5");
  await expect(app.editor).toContainText("never sent");
  await gmail.offline(false);
  await app.advance(60_000);
  await app.settle();
  expect(await sentIn(gmail, s.thread("p5").threadId)).toEqual([]);
  await app.press("Escape", "Escape", "Escape");

  // The build step's drafts.create fails: back to waiting, retried, sent once.
  await gmail.offline(true);
  await openThread(app, "Person 6");
  await app.reply();
  await app.type("built after a drop");
  await app.settle();
  await app.press("Meta+Enter");
  await expect(app.editor).toHaveCount(0);
  await gmail.offline(false);
  await gmail.fault("drafts.create", { kind: "drop" }, { times: 1 });
  await app.advance(DELAY);
  await app.advance(60_000, { stepwise: true });
  await app.settle();
  const creates = await gmail.requests("drafts.create");
  expect(creates.map((r) => r.answer)).toContain("dropped");
  expect(await sentIn(gmail, s.thread("p6").threadId)).toHaveLength(1);
});

test("SND-08 drafts.send with no clear answer never sends twice", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  const rows: { key: string; subject: string; outcome: Outcome; sends: number }[] = [
    { key: "p1", subject: "Person 1", outcome: { kind: "noResponse" }, sends: 1 },
    { key: "p3", subject: "Person 3", outcome: { kind: "drop" }, sends: 2 },
    {
      key: "p5",
      subject: "Person 5",
      outcome: { kind: "statusAfterApply", status: 500 },
      sends: 1,
    },
    { key: "p6", subject: "Person 6", outcome: { kind: "status", status: 503 }, sends: 2 },
  ];
  const run = async (i: number): Promise<void> => {
    const row = rows[i];
    if (!row) return;
    const sendsBefore = await count(gmail, "drafts.send");
    const getsBefore = await count(gmail, "drafts.get");
    await gmail.fault("drafts.send", row.outcome, { times: 1 });
    await sendReply(app, row.subject, `no clear answer ${row.key}`);
    await app.advance(DELAY);
    await app.advance(30_000, { stepwise: true });
    await app.settle();
    const sent = await sentIn(gmail, s.thread(row.key).threadId);
    expect(sent, row.key).toHaveLength(1);
    expect(await count(gmail, "drafts.send"), row.key).toBe(sendsBefore + row.sends);
    expect(await count(gmail, "drafts.get"), row.key).toBeGreaterThan(getsBefore);
    // The row is sent: the conversation shows the message, not a Sending row.
    await expect(app.messages).toHaveCount(2);
    await expect(app.conversation.getByText(/^(Sending|Email delayed)$/)).toHaveCount(0);
    await app.press("Escape");
    await run(i + 1);
  };
  await run(0);
});

test("SND-09 outbox rows across a quit and relaunch", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);

  // Waiting, 3 s in; the clock moves a minute while the app is down.
  await sendReply(app, "Person 3", "across a quit");
  await app.advance(3_000);
  const hold = await gmail.hold("drafts.send");
  await app.restart(async () => {
    await app.advance(60_000);
    expect(await gmail.requests("drafts.send")).toEqual([]);
  });
  await expect(sendToast(app, "Person 3")).toContainText("Email delayed");
  await hold.release();
  await app.settle();
  expect(await sentIn(gmail, s.thread("p3").threadId)).toHaveLength(1);
  expect(await gmail.requests("drafts.send")).toHaveLength(1);

  // Waiting, offline at relaunch: Undo reopens the composer; nothing is sent.
  await sendReply(app, "Person 5", "undone after relaunch");
  await app.restart(async () => {
    await gmail.offline(true);
  });
  await undoOn(sendToast(app, "Person 5")).click();
  await expect(app.conversationTitle).toContainText("Person 5");
  await expect(app.editor).toContainText("undone after relaunch");
  await gmail.offline(false);
  await app.advance(60_000);
  await app.settle();
  expect(await sentIn(gmail, s.thread("p5").threadId)).toEqual([]);
  await app.press("Escape", "Escape", "Escape");

  // Sending with no answer (Gmail sent it), then a crash: settled on launch.
  await gmail.fault("drafts.send", { kind: "noResponse" }, { times: 1 });
  await sendReply(app, "Person 6", "sent before the crash");
  await app.advance(DELAY);
  await app.settle();
  const getsBefore = await count(gmail, "drafts.get");
  await app.crashAndRelaunch();
  await app.settle();
  expect(await count(gmail, "drafts.get")).toBeGreaterThan(getsBefore);
  expect(await sentIn(gmail, s.thread("p6").threadId)).toHaveLength(1);

  // Sent, toast still open, then a quit: the send entry is dropped on launch,
  // so `z` reaches the entry before it.
  const p2Labels = async () =>
    (await gmail.gmail(A)).messages.find((m) => m.id === s.message("p2", 1).id)?.labels ?? [];
  await highlight(app, "Person 2");
  await app.press("e");
  await app.settle();
  expect(await p2Labels()).not.toContain("INBOX");
  await sendReply(app, "Person 1", "delivered");
  await passSendAfter(app);
  await expect(sendToast(app, "Person 1")).toContainText("Sent");
  await app.restart();
  await app.press("z");
  await app.settle();
  const trashed = (await gmail.gmail(A)).messages.filter(
    (m) => m.labels.includes("SENT") && m.labels.includes("TRASH"),
  );
  expect(trashed).toEqual([]);
  expect(await p2Labels()).toContain("INBOX");
  expect(await sentIn(gmail, s.thread("p1").threadId)).toHaveLength(1);
});

test("SND-10 send toasts stay across accounts", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  await sendReply(app, "Person 3", "across accounts");
  await app.press("Control+2");
  await expect(app.row("B 2")).toHaveCount(1);
  const toast = sendToast(app, "Person 3");
  await expect(toast).toBeVisible();
  await undoOn(toast).click();
  await expect(app.conversationTitle).toContainText("Person 3");
  await expect(app.editor).toContainText("across accounts");
  await app.settle();
  expect(await gmail.requests("drafts.send")).toEqual([]);
});

test("SND-11 undoing a waiting send while another composer is open", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);

  // The other composer is touched: kept in Drafts.
  await sendReply(app, "Person 3", "send S");
  await app.compose();
  await app.subject.fill("Other touched");
  await app.settle();
  await undoOn(sendToast(app, "Person 3")).click();
  await expect(app.conversationTitle).toContainText("Person 3");
  await expect(app.editor).toContainText("send S");
  await expect(newMessage(app)).toHaveCount(0);

  // The other composer is untouched: discarded.
  await app.press("Escape");
  await app.press("Meta+Enter");
  await expect(app.editor).toHaveCount(0);
  await expect(undoOn(sendToast(app, "Person 3"))).toBeVisible();
  await app.compose();
  await expect(undoOn(sendToast(app, "Person 3"))).toBeVisible();
  await undoOn(sendToast(app, "Person 3")).click();
  await expect(app.conversationTitle).toContainText("Person 3");
  await expect(app.editor).toContainText("send S");
  await expect(newMessage(app)).toHaveCount(0);

  await app.press("Escape", "Escape", "Escape", "g", "d");
  await app.settle();
  await expect(app.rows).toHaveCount(2);
  await expect(app.row("Other touched")).toHaveCount(1);
  await expect(app.row("send S")).toHaveCount(1);
  expect((await gmail.gmail(A)).drafts).toHaveLength(2);
});

test("SND-12 the sent message shows without waiting for a poll", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  const p3 = s.thread("p3").threadId;
  await sendReply(app, "Person 3", "shown at once");
  await expect(app.conversation.getByText("Sending", { exact: true })).toBeVisible();
  await app.advance(DELAY);
  await app.settle();

  await expect(app.messages).toHaveCount(2);
  await expect(app.conversation.getByText("Sending", { exact: true })).toHaveCount(0);
  const [sent] = await sentIn(gmail, p3);
  expect(sent).toBeDefined();
  const log = await gmail.requests(undefined, { account: A });
  const send = log.findIndex((r) => r.api === "drafts.send");
  const next = log[send + 1];
  const fetched =
    (next?.api === "threads.get" && next.path.endsWith(`/${p3}`)) ||
    (next?.api === "messages.get" && next.path.endsWith(`/${sent?.id ?? "none"}`));
  expect(fetched, `request after drafts.send: ${next?.api} ${next?.path}`).toBe(true);
  expect(log.slice(send).some((r) => r.api === "history.list")).toBe(false);
});

/**
 * From the list: opens the thread and replies "one", which autosaves to
 * Gmail. Then " two" is typed, but its upsert fails, so Gmail keeps "one"
 * while the local draft says "one two". Returns with the composer open.
 */
async function replyAheadOfGmail(app: App, gmail: Control, subject: string): Promise<void> {
  await openThread(app, subject);
  await app.reply();
  await app.type("one");
  await app.settle();
  await gmail.fault("drafts.update", { kind: "status", status: 503 });
  await app.type(" two");
  await app.settle();
}

/** The HTML body of the one message A sent on a Gmail thread. */
async function sentBody(gmail: Control, threadId: string): Promise<string> {
  const sent = await sentIn(gmail, threadId);
  expect(sent).toHaveLength(1);
  return htmlOf(parseMime(sent[0]?.raw ?? ""));
}

test("SND-13 a draft reconcile in the undo window keeps the version that was sent", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  await replyAheadOfGmail(app, gmail, "Person 3");
  await app.press("Meta+Enter");
  await expect(app.editor).toHaveCount(0);
  await gmail.clearFaults();
  // A poll in the undo window reconciles drafts while Gmail still holds "one".
  await gmail.poll();
  await app.settle();
  await app.advance(DELAY);
  await app.settle();
  expect(await sentBody(gmail, s.thread("p3").threadId)).toContain("one two");
  expect((await gmail.gmail(A)).drafts).toEqual([]);
});

test("SND-14 a crash while the send writes the draft still sends the final version", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  await replyAheadOfGmail(app, gmail, "Person 3");
  await app.press("Meta+Enter");
  await expect(app.editor).toHaveCount(0);
  await gmail.clearFaults();
  // The row is sending, and its write of "one two" is in flight when the app dies.
  const write = await gmail.hold("drafts.update");
  await app.advance(DELAY);
  await app.crashAndRelaunch(async () => {
    // The crash dropped the parked write; end the hold.
    await write.release({ kind: "drop" }, { min: 0 });
  });
  await app.settle();
  expect(await sentBody(gmail, s.thread("p3").threadId)).toContain("one two");
  expect((await gmail.gmail(A)).drafts).toEqual([]);
});

test("SND-15 a held undo resolves at once when the send could not reach Gmail", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  await sendReply(app, "Person 3", "held, never written");
  await app.press("Escape");
  // The row is sending, and its write of the Gmail draft is in flight.
  const write = await gmail.hold("drafts.update");
  await app.advance(DELAY);
  const toast = sendToast(app, "Person 3");
  await undoOn(toast).click();
  await expect(toast).toContainText("Undoing…");
  // The write fails, so Gmail never got the send: it resolved as not sent.
  await write.release({ kind: "status", status: 503 });
  await app.settle();
  await expect(app.conversationTitle).toContainText("Person 3");
  await expect(app.editor).toContainText("held, never written");
  await expect(toast).toHaveCount(0);
  await app.advance(60_000, { stepwise: true });
  await app.settle();
  expect(await gmail.requests("drafts.send")).toEqual([]);
  expect(await sentIn(gmail, s.thread("p3").threadId)).toEqual([]);
});

/** Swipes a toast down and away, as a person closes one. */
async function swipeAway(app: App, toast: Locator): Promise<void> {
  const box = await toast.boundingBox();
  if (!box) throw new Error("the toast is not on screen");
  const { mouse } = app.page;
  // Its top left corner: clear of the Undo button, and on screen while the
  // toast is still sliding in.
  const x = box.x + 12;
  const y = box.y + 12;
  await mouse.move(x, y);
  await mouse.down();
  await expect(toast).toHaveAttribute("data-swiping", "true");
  // The first move picks the direction; the toast follows once it has rendered.
  let dy = 4;
  await mouse.move(x, y + dy);
  await expect
    .poll(async () => {
      dy += 20;
      await mouse.move(x, y + dy);
      return toast.getAttribute("data-swiped");
    })
    .toBe("true");
  await mouse.up();
}

test("SND-16 a send toast closed before the send went out takes its undo entry with it once sent", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  const p6Labels = async () =>
    (await gmail.gmail(A)).messages.find((m) => m.id === s.message("p6", 1).id)?.labels ?? [];
  // An older entry: once the send's entry leaves the history, `z` reaches it.
  await highlight(app, "Person 6");
  await app.press("e");
  await app.settle();
  expect(await p6Labels()).not.toContain("INBOX");

  await sendNew(app, "Closed early", "Body of a send whose toast was closed");
  const toast = sendToast(app, "Closed early");
  await expect(toast).toContainText("Sending…");
  await swipeAway(app, toast);
  await expect(toast).toHaveCount(0);
  // The sent message is not stored at once, so the outbox row stays.
  await gmail.fault("threads.get", { kind: "status", status: 503 });
  await app.advance(DELAY);
  await app.settle();
  await gmail.clearFaults();
  const sent = (await gmail.gmail(A)).messages.filter(
    (m) => m.labels.includes("SENT") && m.headers.subject === "Closed early",
  );
  expect(sent).toHaveLength(1);

  // A stray `z` never trashes delivered mail; it undoes the entry before.
  await app.press("z");
  await app.settle();
  const after = (await gmail.gmail(A)).messages.find((m) => m.id === sent[0]?.id);
  expect(after?.labels).not.toContain("TRASH");
  expect(await p6Labels()).toContain("INBOX");
});

test("SND-17 an Undo that reaches the core only after the send went out opens no composer", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  await sendReply(app, "Person 3", "undone too late");
  await app.press("Escape");
  await app.settle();
  const toast = sendToast(app, "Person 3");
  await expect(toast).toContainText("Sending…");

  // The Undo leaves the page while the row is waiting, but its invoke is slow.
  const parked: Route[] = [];
  await app.page.route("**/invoke/submit", async (route) => {
    if (parked.length === 0) parked.push(route);
    else await route.continue();
  });
  await undoOn(toast).click();
  await expect.poll(() => parked.length).toBe(1);

  // Meanwhile the send goes out. (`app.advance` would wait for the invoke.)
  const send = await gmail.hold("drafts.send");
  await gmail.advanceServerClock(DELAY);
  await send.release();
  await expect(toast).toContainText("Sent");

  // The Undo arrives: the row is sent, so it trashes the message. No
  // composer opens on the draft the send used up.
  await parked[0]?.continue();
  await app.settle();
  await app.page.unroute("**/invoke/submit");
  const [sent] = await sentIn(gmail, s.thread("p3").threadId);
  expect(sent?.labels).toContain("TRASH");
  await expect(app.editor).toHaveCount(0);
  await expect(app.conversationTitle).toHaveCount(0);
});

test("SND-18 a send while the draft's first upsert is in flight goes out once and leaves no draft", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  const p3 = s.thread("p3").threadId;
  const create = await gmail.hold("drafts.create", { nth: 1 });
  await openThread(app, "Person 3");
  await app.reply();
  await app.type("sent while saving");
  await expect
    .poll(async () => (await gmail.requests("drafts.create")).some((r) => r.held))
    .toBe(true);
  await app.press("Meta+Enter");
  await expect(app.editor).toHaveCount(0);
  // The send's time comes while the autosave's create is still in flight.
  await passSendAfter(app);
  await create.release();
  await app.settle();
  await expect(sendToast(app, "Person 3")).toContainText("Sent");
  // The toast closes, and a poll reconciles drafts.
  await app.advance(5_000);
  await gmail.poll();
  await app.settle();

  expect(await sentBody(gmail, p3)).toContain("sent while saving");
  expect((await gmail.gmail(A)).drafts).toEqual([]);
  await app.press("Escape", "g", "d");
  await expect(app.row("Person 3")).toHaveCount(0);
});

test("SND-19 a send the core refuses says it was not sent and brings the draft back", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  // The core stores an address the composer took for valid, as a UI out of
  // step with the core's check would.
  const submit = /\/invoke\/submit$/;
  await app.page.route(submit, (route) =>
    route.continue({
      postData: (route.request().postData() ?? "").replaceAll("kim@kite.test", "kim@kite.c"),
    }),
  );
  await app.compose();
  await app.type("kim@kite.test,");
  await app.subject.fill("Refused");
  await app.editor.click();
  await app.type("Kept for later");
  await app.settle();
  await app.press("Meta+Enter");
  await app.settle();
  await app.page.unroute(submit);

  await expect(
    app.errorToasts.filter({ hasText: "Not sent: kim@kite.c isn't a valid email address" }),
  ).toHaveCount(1);
  // Idle, and the draft is a new message: the `c` screen opens with it.
  await expect(newMessage(app)).toBeVisible();
  await expect(app.subject).toHaveValue("Refused");
  await expect(app.editor).toContainText("Kept for later");
  await expect(app.invalidChips("To")).toHaveAttribute("data-chip", "kim@kite.c");
  await app.advance(DELAY);
  await app.settle();
  await expect(app.toast("Sending…")).toHaveCount(0);
  expect(await gmail.requests("drafts.send")).toEqual([]);
});
