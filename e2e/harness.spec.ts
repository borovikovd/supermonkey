// Checks the harness itself: seeds, settle, holds and faults, the request log,
// the clock, restart and crash, focus, links and remote images, Gmail-side
// drafts, OAuth, introspection, and the UI helpers. Each case is short; the
// product's behavior is covered by the other specs.
import { expect, test } from "./fixtures.ts";
import { files } from "./files.ts";
import { htmlOf, parseMime, shape } from "./mime.ts";
import * as seeds from "./seeds.ts";
import { A, B } from "./seeds.ts";

test("standard seed: splits, sendAs, and keyed handles", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  expect(await app.splitHeader()).toBe("Important 16 · Other 7");
  const sections = await app.sections();
  expect(sections.map((x) => x.name)).toEqual(["Important", "Other"]);
  expect(sections[0]?.subjects.slice(0, 3)).toEqual(["Person 1", "Person 2", "Person 3"]);
  await expect(app.unread).toHaveCount(9);
  // B's avatar initial comes from its primary sendAs name, "Zed Bolt".
  await expect(app.account(B)).toContainText("Z");
  const a = await gmail.gmail(A);
  expect(a.sendAs.map((x) => x.email)).toEqual([A, seeds.A_ALIAS]);
  expect(s.thread("collide").threadId).toBe(s.thread("p1").threadId);
  expect(s.thread("plan").messages).toHaveLength(12);
  // Browser and core agree on the time: the anchor.
  expect(await app.page.evaluate(() => Date.now())).toBe(seeds.ANCHOR);
  expect(await app.now()).toBe(seeds.ANCHOR);
});

test("hold parks a request; settle does not wait for it; the log records it", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  const hold = await gmail.hold("messages.batchModify");
  await app.press("j", "j", "e");
  await expect(app.row("Person 3")).toHaveCount(0);
  await app.settle();
  const [held] = await gmail.requests("messages.batchModify");
  expect(held).toMatchObject({ held: true, answer: null, account: A, method: "POST" });
  expect(held?.body).toEqual({
    ids: [s.message("p3", 1).id],
    addLabelIds: [],
    removeLabelIds: ["INBOX", "UNREAD"],
  });
  expect(await hold.release()).toBe(1);
  await app.settle();
  const [done] = await gmail.requests("messages.batchModify");
  expect(done).toMatchObject({ held: false, answer: 204 });
  const p3 = (await gmail.gmail(A)).messages.find((m) => m.id === s.message("p3", 1).id);
  expect(p3?.labels).not.toContain("INBOX");
});

test("settle skips a held request with a second action queued behind it", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  const hold = await gmail.hold("messages.batchModify");
  await app.press("e");
  await app.press("j", "e");
  await app.settle();
  expect((await gmail.requests("messages.batchModify")).map((r) => r.held)).toEqual([true]);
  expect(await hold.release()).toBe(1);
  await app.settle();
  expect((await gmail.requests("messages.batchModify")).map((r) => r.held)).toEqual([false, false]);
});

test("a held request can be released with a fault", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  const hold = await gmail.hold("messages.batchModify");
  // Called before the request exists: it waits until the hold has parked it.
  const released = hold.release({ kind: "status", status: 400, reason: "invalidArgument" });
  await app.press("j", "e");
  expect(await released).toBe(1);
  await app.settle();
  // A rejection rolls the action back and says so.
  await expect(app.row("Person 2")).toHaveCount(1);
  await expect(app.errorToasts).toHaveCount(1);
});

test("faults match by account and apply after the work is done", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  // Only B's requests: A's archive is unaffected.
  await gmail.fault({ api: "messages.batchModify", account: B }, { kind: "status", status: 503 });
  await gmail.fault(
    "messages.batchModify",
    { kind: "statusAfterApply", status: 500 },
    { account: A, times: 1 },
  );
  await app.press("e");
  await app.settle();
  // Gmail applied it, though the app heard 500 and is backing off.
  const p1 = (await gmail.gmail(A)).messages.find((m) => m.id === s.message("p1", 1).id);
  expect(p1?.labels).not.toContain("INBOX");
  await app.advance(2_000);
  await app.settle();
  const answers = (await gmail.requests("messages.batchModify")).map((r) => r.answer);
  expect(answers).toEqual([500, 204]);
});

test("polls run on app time; stepwise advance runs every retry", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  const polls = async () => (await gmail.requests("history.list", { account: A })).length;
  const before = await polls();
  await gmail.deliver({ account: A, from: "New <new@x.test>", subject: "Fresh", poll: false });
  await app.advance(44_000);
  expect(await polls()).toBe(before);
  await expect(app.row("Fresh")).toHaveCount(0);
  await app.advance(1_000);
  expect(await polls()).toBe(before + 1);
  expect(await app.page.evaluate(() => Date.now())).toBe(seeds.ANCHOR + 45_000);
  await expect(app.row("Fresh")).toHaveCount(1);

  await gmail.fault("messages.batchModify", { kind: "status", status: 503 }, { times: 3 });
  await app.press("e");
  await app.settle();
  await app.advance(60_000, { stepwise: true });
  const tries = await gmail.requests("messages.batchModify");
  expect(tries.map((r) => r.answer)).toEqual([503, 503, 503, 204]);
  const gaps = tries.slice(1).map((r, i) => r.at - (tries[i]?.at ?? 0));
  // 1 s doubling, with ±20% jitter.
  expect(gaps[0]).toBeGreaterThanOrEqual(800);
  expect(gaps[1]).toBeGreaterThan(gaps[0] ?? 0);
  expect(gaps[2]).toBeGreaterThan(gaps[1] ?? 0);
});

test("a quit closes the composer first; Gmail and the clock change while down", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  await app.press("j", "j", "Enter");
  await app.reply();
  await app.type("Written before the quit");
  const quitting = Date.now();
  let quitMs = Infinity;
  await app.restart(async () => {
    quitMs = Date.now() - quitting;
    await gmail.deliver({
      account: A,
      from: "Late <late@x.test>",
      subject: "While down",
      poll: false,
    });
    await app.advance(60_000);
  });
  // The page answered quit-requested: the core did not wait out its 10 s fallback.
  expect(quitMs).toBeLessThan(5_000);
  expect(await app.now()).toBe(seeds.ANCHOR + 60_000);
  await expect(app.row("While down")).toHaveCount(1);
  const drafts = (await gmail.gmail(A)).drafts;
  expect(drafts).toHaveLength(1);
  expect(drafts[0]?.message.raw).toContain("Written before the quit");
});

test("a crash skips the composer close; launch applies the untouched rule", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  await app.compose();
  await app.subject.fill("Kept after a crash");
  await app.settle();
  await app.crashAndRelaunch();
  await app.press("g", "d");
  await expect(app.row("Kept after a crash")).toHaveCount(1);
  await app.press("c");
  await app.crashAndRelaunch();
  await app.press("g", "d");
  await expect(app.rows).toHaveCount(1);
});

test("sleep moves the wall clock without firing timers; the next poll notices", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  const polls = async () => (await gmail.requests("history.list", { account: A })).length;
  const before = await polls();
  await app.sleep(10 * 60_000);
  await app.settle();
  expect(await polls()).toBe(before);
  expect(await app.page.evaluate(() => Date.now())).toBe(seeds.ANCHOR + 10 * 60_000);
  await app.sleepAndWake(0);
  expect(await polls()).toBe(before + 1);
});

test("focus: the page follows the core; focus after a notification opens it", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  await app.focus(false);
  expect(await app.page.evaluate(() => document.hasFocus())).toBe(false);
  const d = await gmail.deliver({ account: B, from: "Una <una@u.test>", subject: "For Ben" });
  await app.settle();
  const { notifications } = await gmail.platform();
  expect(notifications).toEqual([
    expect.objectContaining({ account: B, thread: d.threadId, title: "Una" }),
  ]);
  expect(await app.focus(true)).toEqual({ account: B, thread: d.threadId });
  expect(await app.page.evaluate(() => document.hasFocus())).toBe(true);
  await expect(app.highlighted).toContainText("For Ben");
  await gmail.clearRecorded();
  expect((await gmail.platform()).notifications).toEqual([]);
});

test("links open through the core's open_url; remote images hit the image host", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  await app.row("Links").click();
  await expect
    .poll(async () => (await gmail.hits()).map((h) => h.path))
    .toEqual(expect.arrayContaining(["/img/pixel.png", "/img/bg.png"]));
  // What MessageFrame sends for a link click in web mode.
  const open = (url: string) =>
    app.page.evaluate(
      async (u) =>
        (
          await fetch("/invoke/open_url", {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: JSON.stringify({ url: u }),
          })
        ).status,
      url,
    );
  expect(await open("https://example.org/a")).toBe(200);
  expect(await open("javascript:alert(1)")).toBe(400);
  expect((await gmail.platform()).openedUrls).toEqual(["https://example.org/a"]);
  // Requests to evil.test reach the sink instead of failing DNS.
  await app.page.evaluate(() => {
    new Image().src = "http://evil.test/probe.png";
  });
  await expect
    .poll(async () => (await gmail.hits()).map((h) => h.path))
    .toContain("/sink/evil.test/probe.png");
  expect(app.popups).toEqual([]);
});

test("removing an account revokes its token and deletes its stored token", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  const token = (await gmail.tokens()).find((k) => k.account === B)?.token;
  await app.press("Meta+,");
  await app.sheet("Settings").getByRole("button", { name: "Remove account" }).nth(1).click();
  await app.sheet("Settings").getByRole("button", { name: "Remove", exact: true }).click();
  await app.settle();
  expect(await gmail.revocations()).toEqual([{ account: B, token }]);
  expect((await gmail.tokens()).map((k) => k.account)).toEqual([A]);
});

test("the Gmail search waits for a 300 ms pause in app time", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  const searches = async () =>
    (await gmail.requests("threads.list")).filter((r) => r.params.q?.startsWith("inv"));
  await app.press("/");
  await app.type("inv");
  await app.advance(299);
  expect(await searches()).toEqual([]);
  await app.advance(1);
  expect((await searches()).map((r) => r.params.q)).toEqual(["inv"]);
  await expect(app.row("Invoice 2019")).toHaveCount(1);
});

const OTHER_SEEDS = {
  empty: seeds.empty,
  drafts: seeds.drafts,
  contacts: seeds.contacts,
  splits: seeds.splits,
  security: seeds.security,
  bulk1000: seeds.bulk1000,
};

for (const [name, seed] of Object.entries(OTHER_SEEDS)) {
  test(`the ${name} seed opens settled`, async ({ app, gmail }) => {
    if (name === "bulk1000") test.slow();
    const t0 = Date.now();
    await seed(gmail, app);
    test.info().annotations.push({ type: "open", description: `${Date.now() - t0} ms` });
    // Read once, without retrying: the seed returns only after its mail is on screen.
    const subjects = await app.subjects();
    if (name === "empty") {
      expect(subjects).toEqual([]);
      expect(await app.account(A).count()).toBe(1);
    }
    if (name === "drafts") expect(await app.splitHeader()).toBe("Important 16 · Other 7");
    if (name === "contacts") {
      expect(subjects).toEqual(expect.arrayContaining(["From Sue", "To the alias"]));
    }
    if (name === "splits") expect(subjects).toEqual(expect.arrayContaining(["X marks", "Y knot"]));
    if (name === "security") expect(subjects).toContain("Hostile");
    if (name === "bulk1000") expect(await app.splitHeader()).toMatch(/^Important 999( · |$)/);
  });
}

test("Gmail-side drafts, deleted threads, and expired history", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  const d = await gmail.createDraft(A, {
    subject: "Re: Person 3",
    threadId: s.thread("p3").threadId,
    inReplyTo: s.message("p3", 1).messageId,
    html: "<p>from web</p>",
  });
  await gmail.poll();
  await app.settle();
  await app.press("g", "d");
  await expect(app.row("from web")).toHaveCount(1);
  await gmail.updateDraft(A, d.id, {
    subject: "Re: Person 3",
    threadId: d.threadId,
    html: "<p>edited on the phone</p>",
  });
  await gmail.poll();
  await app.settle();
  await expect(app.row("edited on the phone")).toHaveCount(1);
  await gmail.sendDraft(A, d.id, { poll: true });
  await app.settle();
  await expect(app.rows).toHaveCount(0);

  await app.press("g", "i");
  await gmail.deleteThread(A, s.thread("p5").threadId);
  await gmail.expireHistory(A);
  await gmail.fault(
    { api: "threads.list", query: { includeSpamTrash: "true" } },
    { kind: "omitThreads", threads: [s.thread("p2").threadId] },
  );
  await gmail.poll();
  await app.settle();
  await expect(app.row("Person 5")).toHaveCount(0);
  // Left out of the listing but still in Gmail: kept.
  await expect(app.row("Person 2")).toHaveCount(1);
  const resync = (await gmail.requests("threads.list", { account: A })).find(
    (r) => r.params.includeSpamTrash === "true",
  );
  expect(resync?.params).toMatchObject({ maxResults: "500" });
  expect(resync?.answer).toBe(200);
});

test("sign-in: a denial stores nothing; the token file lists tokens; no token in the database", async ({
  app,
  gmail,
}) => {
  await seeds.none(gmail, app);
  await expect(app.firstRunButton).toBeVisible();
  await gmail.nextSignIn({ deny: true });
  await app.firstRunButton.click();
  await expect(app.page.getByText(/access_denied/)).toBeVisible();
  expect(await gmail.tokens()).toEqual([]);
  await gmail.nextSignIn({ email: A });
  await app.firstRunButton.click();
  await expect(app.account(A)).toBeVisible();
  await app.settle();
  const stored = await gmail.tokens();
  expect(stored.map((t) => t.account)).toEqual([A]);
  expect(await gmail.tokenFileMode(), "only the user can read the token file").toBe("600");
  const auth = (await gmail.platform()).openedUrls.at(-1) ?? "";
  expect(new URL(auth).searchParams.get("scope")).toBe(
    "https://www.googleapis.com/auth/gmail.modify https://www.googleapis.com/auth/gmail.settings.basic",
  );
  expect(await gmail.dbContains(stored[0]?.token ?? "missing")).toBe(false);
  expect(await gmail.dbContains("Q4 plan")).toBe(true);
});

test("UI helpers: paste, chips, file chooser", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  await app.compose();
  await app.paste("a@x.test, Bob <b@y.test>; c@z.test\nd@w.test");
  await expect(app.chips("To")).toHaveCount(4);
  await expect(app.chips("To").nth(1)).toHaveText(/^Bob/);
  await app.type("kim@kite,");
  await expect(app.invalidChips("To")).toHaveCount(1);
  await app.chooseFiles(() => app.press("Meta+Shift+a"), [files.reportPdf]);
  await expect(app.page.getByText("report.pdf")).toBeVisible();
});

test("UI handles: selection, unread, the selected message, the active split", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  await app.press("j", "x");
  await expect(app.selected).toHaveCount(1);
  await expect(app.selected).toContainText("Person 2");
  await expect(app.activeSplit).toHaveText("Important");
  const probe = await app.watchRows();
  const key = await app.rowKey("Person 1");
  await app.press("Escape", "k", "e");
  await gmail.deliver({ account: A, from: "Ping <ping@x.test>", subject: "Ping" });
  await app.settle();
  const added = await probe.added();
  expect(added).toContain(await app.rowKey("Ping"));
  expect(added).not.toContain(key);
  await app.row("Q4 plan").click();
  await expect(app.selectedMessage).toHaveCount(1);
  await expect(app.selectedMessage).toHaveAttribute("data-message", /\d+/);
  expect(await app.messages.last().getAttribute("aria-current")).toBe("true");
});

test("MIME: a sent reply's part tree", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  await app.press("j", "j", "Enter");
  await app.reply();
  // Sent before the autosave runs: the send carries the typed text itself.
  await app.type("Thanks for the note");
  await app.press("Meta+Enter");
  await app.advance(10_000);
  await app.settle();
  const sent = (await gmail.gmail(A)).messages.find(
    (m) => m.threadId === s.thread("p3").threadId && m.labels.includes("SENT"),
  );
  const tree = parseMime(sent?.raw ?? "");
  expect(shape(tree)).toBe("multipart/related[multipart/alternative[text/plain,text/html]]");
  expect(htmlOf(tree)).toContain("Thanks for the note");
  expect(tree.headers["in-reply-to"]).toBe(s.message("p3", 1).messageId);
});

test("bulk seed at the fast quota; the production budget is a knob", async ({ app, gmail }) => {
  await gmail.quota(true);
  await gmail.quota(false);
  await seeds.bulk(gmail, app);
  expect(await app.splitHeader()).toBe("Important 40 · Other 20");
});
