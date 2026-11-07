// First run, adding and switching accounts, avatars, removal, Sign in again,
// and isolation between accounts.
import type { Locator } from "@playwright/test";

import type { App } from "./app.ts";
import type { Control } from "./control.ts";
import { expect, test } from "./fixtures.ts";
import * as seeds from "./seeds.ts";
import { A, B, C } from "./seeds.ts";

/** A's Inbox in the `standard` seed, sorted. */
const A_INBOX = [
  "Alias note",
  "Big sale",
  "Budget draft",
  "Contract",
  "Digest: daily",
  "Digest: week 43",
  "Digest: week 44",
  "Fold check",
  "Forum thread",
  "Guild meetup",
  "Latest rule",
  "Links",
  "New follower",
  "Offsite venue",
  "Person 1",
  "Person 2",
  "Person 3",
  "Person 4",
  "Person 5",
  "Person 6",
  "Q4 plan",
  "Ticket #42",
  "Your receipt",
].toSorted();

/** Senders of B's Inbox rows when B is added after A: b1 to b4, and `collide`. */
const B_INBOX_SENDERS = ["Bea Blue", "Bo Brown", "Bill Black", "Bree Bell", "Cole Lide"];

/** The labels of a message in Gmail. */
async function labelsOf(gmail: Control, email: string, id: string): Promise<string[]> {
  const m = (await gmail.gmail(email)).messages.find((x) => x.id === id);
  if (!m) throw new Error(`no message ${id} in ${email}`);
  return m.labels;
}

/** The unread count on an account's avatar; hidden at zero. */
const badge = (app: App, email: string): Locator =>
  app.account(email).locator("span", { hasText: /^\d+\+?$/ });

/** Every avatar in the rail, top to bottom, by its title's address. */
async function avatarOrder(app: App): Promise<string[]> {
  const titles = await app.page
    .getByRole("navigation")
    .locator("button[title*='(Ctrl+']")
    .evaluateAll((els) => els.map((el) => el.getAttribute("title") ?? ""));
  return titles.map((t) => t.split(" ")[0] ?? "");
}

/**
 * Opens a conversation, then replies with `r`. Opening first keeps these cases
 * off a separate bug: `r` on the list can start more than one draft
 * (`whenLoaded` in src/compose/start.ts runs once per queued frame).
 */
async function replyTo(app: App, subject: string): Promise<void> {
  await app.row(subject).click();
  await expect(app.conversationTitle).toHaveText(subject);
  await app.settle();
  await app.reply("r");
}

/** Highlights the row containing `text` with j / k from the current highlight. */
async function highlight(app: App, text: string): Promise<void> {
  const at = await app.rows.evaluateAll(
    (rows, t) => ({
      from: rows.findIndex((r) => r.getAttribute("aria-selected") === "true"),
      to: rows.findIndex((r) => (r.textContent ?? "").includes(t)),
    }),
    text,
  );
  if (at.to < 0) throw new Error(`no row with ${text}`);
  const delta = at.to - at.from;
  await app.press(...Array.from({ length: Math.abs(delta) }, () => (delta > 0 ? "j" : "k")));
  await expect(app.highlighted).toContainText(text);
}

/** Runs the palette command `name`. */
async function command(app: App, name: string): Promise<void> {
  await app.press("Meta+k");
  await expect(app.palette).toBeVisible();
  await app.type(name);
  await expect(app.palette.locator("[cmdk-item]").first()).toContainText(name);
  await app.press("Enter");
}

/** An account's block in the Settings sheet. */
function settingsBlock(app: App, email: string): Locator {
  return app
    .sheet("Settings")
    .locator("div")
    .filter({ has: app.page.getByText(email, { exact: true }) })
    .filter({ has: app.page.getByRole("button", { name: "Remove account" }) })
    .last();
}

test("ACC-01 first run adds the first account; its token goes to a file only the user can read", async ({
  app,
  gmail,
}) => {
  await seeds.none(gmail, app);
  // The first-run screen: no rail, no list.
  await expect(app.firstRunButton).toBeVisible();
  await expect(app.page.getByRole("navigation")).toHaveCount(0);
  await expect(app.list).toHaveCount(0);

  await gmail.nextSignIn({ email: A });
  await app.firstRunButton.click();
  await expect(app.account(A)).toBeVisible();
  await app.settle();

  // A's Inbox shows the same threads as Gmail's INBOX.
  await expect(app.rows).toHaveCount(A_INBOX.length);
  expect((await app.subjects()).toSorted()).toEqual(A_INBOX);

  const stored = await gmail.tokens();
  expect(stored.map((t) => t.account)).toEqual([A]);
  expect(await gmail.tokenFileMode(), "only the user can read the token file").toBe("600");
  // The consent page opened in the system browser, with a loopback redirect, PKCE, and one scope.
  const { openedUrls } = await gmail.platform();
  expect(openedUrls).toHaveLength(1);
  const auth = new URL(openedUrls[0] ?? "");
  expect(auth.searchParams.get("redirect_uri")).toMatch(/^http:\/\/127\.0\.0\.1:\d+/);
  expect(auth.searchParams.get("code_challenge")).toBeTruthy();
  expect(auth.searchParams.get("code_challenge_method")).toBe("S256");
  expect(auth.searchParams.get("scope")).toBe(
    "https://www.googleapis.com/auth/gmail.modify https://www.googleapis.com/auth/gmail.settings.basic",
  );
  expect(app.popups).toEqual([]);
  expect(await gmail.dbContains(stored[0]?.token ?? "missing")).toBe(false);
});

test("ACC-02 add a second account from the palette; switch with Ctrl+1 and Ctrl+2", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app, { connect: ["A"] });
  await expect(app.account(B)).toHaveCount(0);

  await gmail.nextSignIn({ email: B });
  await command(app, "Add account");
  await expect(app.account(B)).toBeVisible();
  await app.settle();

  // Rail order is the order added. B's initial comes from its sendAs name, Zed Bolt.
  expect(await avatarOrder(app)).toEqual([A, B]);
  await expect(app.account(B)).toHaveText(/^Z/);
  await expect(app.account(A)).toHaveText(/^A/);
  expect((await gmail.tokens()).map((k) => k.account).toSorted()).toEqual([A, B]);
  // Avatar hues, in the order added.
  const hue = (email: string) =>
    app.account(email).evaluate((el) => {
      const span = el.querySelector("span");
      return span ? getComputedStyle(span).color : "";
    });
  expect(await hue(A)).toBe("rgb(113, 76, 182)");
  expect(await hue(B)).toBe("rgb(214, 71, 138)");

  await app.press("Control+2");
  await expect(app.rows).toHaveCount(B_INBOX_SENDERS.length);
  await Promise.all(B_INBOX_SENDERS.map((sender) => expect(app.row(sender)).toHaveCount(1)));
  await expect(app.row("Person 1")).toHaveCount(0);
  await app.press("Control+1");
  await expect(app.rows).toHaveCount(A_INBOX.length);
  expect((await app.subjects()).toSorted()).toEqual(A_INBOX);

  // Out of range: nothing happens.
  await app.press("Control+3");
  await app.settle();
  await expect(app.rows).toHaveCount(A_INBOX.length);
  expect((await app.subjects()).toSorted()).toEqual(A_INBOX);
});

test("ACC-03 switching closes overlays and returns each account to its own place", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  // Two more starred threads on A, so Starred has a second row.
  await gmail.modify(A, s.latest("p2").id, ["STARRED"], []);
  await gmail.modify(A, s.latest("p4").id, ["STARRED"], [], { poll: true });
  await app.settle();

  // A: Starred, second row.
  await app.press("g", "s");
  await expect(app.row("Trip ideas")).toHaveCount(1);
  await expect(app.highlighted).toContainText("Person 2");
  await app.press("j");
  await expect(app.highlighted).toContainText("Person 4");

  // B's first visit: Inbox, top row highlighted.
  await app.press("Control+2");
  await expect(app.splitTitles().first()).toBeVisible();
  await expect(app.highlighted).toContainText("Bea Blue");
  await app.press("j", "Enter");
  await expect(app.conversationTitle).toHaveText("B 2");
  await app.press("Meta+k");
  await expect(app.palette).toBeVisible();

  // Back to A: the palette closes; A is on Starred, second row.
  await app.press("Control+1");
  await expect(app.palette).toHaveCount(0);
  await expect(app.row("Trip ideas")).toHaveCount(1);
  await expect(app.highlighted).toContainText("Person 4");
  await app.press("/");
  await app.type("venue");
  await expect(app.page.getByPlaceholder("Search")).toHaveValue("venue");

  // Back to B: search closes; B's Inbox list with the thread it had open highlighted.
  await app.press("Control+2");
  await expect(app.page.getByPlaceholder("Search")).toHaveCount(0);
  await expect(app.list).toBeVisible();
  await expect(app.conversationTitle).toHaveCount(0);
  await expect(app.highlighted).toContainText("Bo Brown");

  // A click on A's avatar: A's last view (Starred, SPEC Switching), second row.
  await app.account(A).click();
  await expect(app.page.getByPlaceholder("Search")).toHaveCount(0);
  await expect(app.row("Trip ideas")).toHaveCount(1);
  await expect(app.highlighted).toContainText("Person 4");

  // The palette's Switch account: B's list again.
  await command(app, "Switch account");
  const switcher = app.page.getByRole("dialog", { name: "Switch account" });
  await expect(switcher).toBeVisible();
  await switcher.locator("[cmdk-item]", { hasText: B }).click();
  await expect(app.palette).toHaveCount(0);
  await expect(app.list).toBeVisible();
  await expect(app.highlighted).toContainText("Bo Brown");
});

test("ACC-04 switching with an open composer works like a second Esc", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  const p1 = s.thread("p1").threadId;
  const draftStub = app.conversation.getByRole("button", { name: /Draft/ });
  const roundTrip = async () => {
    await app.press("Control+2");
    await expect(app.highlighted).toContainText("Bea Blue");
    await app.press("Control+1");
    await app.settle();
  };
  const draftsIn = async (email: string) => (await gmail.gmail(email)).drafts;

  // A fresh r on p1, untouched: discarded.
  await replyTo(app, "Person 1");
  await roundTrip();
  await expect(app.list).toBeVisible();
  expect(await draftsIn(A)).toEqual([]);
  await app.press("g", "d");
  await expect(app.page.getByText("Nothing here.")).toBeVisible();
  await app.press("g", "i");
  await app.row("Person 1").click();
  await expect(app.conversationTitle).toHaveText("Person 1");
  await app.settle();
  await expect(draftStub).toHaveCount(0);

  // A fresh r on p1 with a typed body: kept in A, not in B.
  await app.reply("r");
  await app.type("Kept across the switch");
  await app.press("Control+2");
  await app.settle();
  await app.press("g", "d");
  await expect(app.page.getByText("Nothing here.")).toBeVisible();
  expect(await draftsIn(B)).toEqual([]);
  await app.press("g", "i");
  await app.press("Control+1");
  await app.settle();
  const aDrafts = await draftsIn(A);
  expect(aDrafts).toHaveLength(1);
  expect(aDrafts[0]?.message.threadId).toBe(p1);
  expect(aDrafts[0]?.message.raw).toContain("Kept across the switch");
  await app.row("Person 1").click();
  await expect(app.conversationTitle).toHaveText("Person 1");
  await expect(draftStub).toHaveCount(1);
  await app.press("Escape");
  await app.press("g", "d");
  await expect(app.row("Kept across the switch")).toHaveCount(1);

  // c with a typed subject: kept in A's Drafts.
  await app.compose();
  await app.subject.fill("Subject kept");
  await roundTrip();
  await expect(app.row("Subject kept")).toHaveCount(1);
  await expect(app.rows).toHaveCount(2);
  expect((await draftsIn(A)).map((d) => d.message.headers.subject)).toContain("Subject kept");

  // The resumed draft, unchanged: kept.
  await app.row("Subject kept").click();
  await expect(app.page.getByRole("heading", { name: "New Message" })).toBeVisible();
  await app.settle();
  await roundTrip();
  await expect(app.row("Subject kept")).toHaveCount(1);
  await expect(app.rows).toHaveCount(2);
  expect(await draftsIn(A)).toHaveLength(2);
  expect(await draftsIn(B)).toEqual([]);
});

test("ACC-05 avatar unread counts", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  await expect(badge(app, A)).toHaveText("9");
  await expect(badge(app, B)).toHaveText("2");

  // Reading b1 and b2 hides B's badge.
  await app.press("Control+2");
  await expect(app.highlighted).toContainText("Bea Blue");
  await app.press("Enter");
  await expect(app.messages.last()).toContainText("Bea Blue");
  await app.press("j");
  await expect(app.conversationTitle).toHaveText("B 2");
  await app.settle();
  await expect(badge(app, B)).toHaveCount(0);

  // Two unread messages in one new thread count once.
  await app.press("Escape", "Control+1");
  await gmail.deliverMany([
    { account: B, from: "Una Two <una@two.test>", subject: "Two in one", poll: false },
    { account: B, from: "Una Two <una@two.test>", subject: "Re: Two in one", after: 0 },
  ]);
  await app.settle();
  await expect(badge(app, B)).toHaveText("1");

  // Unread mail outside Inbox does not count.
  await gmail.deliver({
    account: B,
    from: "Out Side <out@side.test>",
    subject: "Not in Inbox",
    labels: ["UNREAD", "CATEGORY_PERSONAL"],
  });
  await app.settle();
  await expect(badge(app, B)).toHaveText("1");
  await expect(badge(app, A)).toHaveText("9");
});

test("ACC-06 accounts off screen keep syncing, notifying, and sending", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  await expect(badge(app, A)).toHaveText("9");
  await replyTo(app, "Person 2");
  await app.type("Sent while away");
  await app.settle();
  await app.press("Meta+Enter");
  await expect(app.toast("Sending")).toBeVisible();
  await app.press("Control+2");
  await expect(app.highlighted).toContainText("Bea Blue");
  await expect(app.toast("Sending")).toBeVisible();

  await gmail.deliver({ account: A, from: "Nia New <nia@new.test>", subject: "While on B" });
  // In two steps: a single 10 s jump would also fire the Sent toast's own close timer.
  await app.advance(9_999);
  await expect(app.toast("Sending")).toBeVisible();
  await app.advance(1);
  await app.settle();

  const sent = (await gmail.gmail(A)).messages.filter(
    (m) => m.threadId === s.thread("p2").threadId && m.labels.includes("SENT"),
  );
  expect(sent).toHaveLength(1);
  expect(sent[0]?.raw).toContain("Sent while away");
  await expect(app.toast(/^Sent/)).toBeVisible();
  await expect(app.highlighted).toContainText("Bea Blue");
  const { notifications } = await gmail.platform();
  expect(notifications).toHaveLength(1);
  expect(notifications[0]?.account).toBe(A);
  expect(notifications[0]?.body).toContain(A);
  await expect(badge(app, A)).toHaveText("10");

  // An archive made offline on A replays while B is on screen.
  await app.press("Control+1");
  await gmail.offline(true);
  await highlight(app, "Person 1");
  await app.press("e");
  await expect(app.row("Person 1")).toHaveCount(0);
  await app.press("Control+2");
  await expect(app.highlighted).toContainText("Bea Blue");
  await gmail.offline(false);
  await app.settle();
  expect(await labelsOf(gmail, A, s.latest("p1").id)).not.toContain("INBOX");
  await expect(app.highlighted).toContainText("Bea Blue");
});

test("ACC-07 remove an account", async ({ app, gmail }) => {
  test.slow();
  const s = await seeds.standard(gmail, app);
  const settings = app.sheet("Settings");
  const tokens = Object.fromEntries((await gmail.tokens()).map((k) => [k.account, k.token]));

  // d: Cancel changes nothing.
  await app.press("Meta+,");
  await settingsBlock(app, B).getByRole("button", { name: "Remove account" }).click();
  await settings.getByRole("button", { name: "Cancel" }).click();
  await expect(settings.getByRole("button", { name: "Remove", exact: true })).toHaveCount(0);
  await app.press("Escape");
  await app.settle();
  expect(await avatarOrder(app)).toEqual([A, B]);
  expect(await gmail.tokens()).toHaveLength(2);
  expect(await gmail.revocations()).toEqual([]);

  // b: remove A while on screen, with 2 queued actions and 1 waiting send.
  await replyTo(app, "Person 2");
  await app.type("Never to be sent");
  await app.settle();
  await app.press("Meta+Enter");
  await app.settle();
  await app.press("Escape");
  await gmail.offline(true);
  await app.press("g", "i");
  await expect(app.highlighted).toContainText("Person 1");
  await app.press("e");
  await expect(app.highlighted).toContainText("Person 2");
  await app.press("s");
  await app.settle();
  await app.press("Meta+,");
  await settingsBlock(app, A).getByRole("button", { name: "Remove account" }).click();
  const confirmA = settings.getByText(/^Remove ana@acme\.test\?/);
  await expect(confirmA).toBeVisible();
  await expect(confirmA).toContainText(/\b2\b[^.]*actions/);
  await expect(confirmA).toContainText(/\b1\b[^.]*send/);
  await settings.getByRole("button", { name: "Remove", exact: true }).click();
  await expect(app.account(A)).toHaveCount(0);
  // The next account takes its place.
  await app.press("Escape");
  await expect(app.highlighted).toContainText("Bea Blue");
  await gmail.offline(false);
  await app.advance(10_000);
  await app.settle();
  const a = await gmail.gmail(A);
  const msg = (key: string) => a.messages.find((m) => m.id === s.latest(key).id);
  expect(msg("p1")?.labels).toContain("INBOX");
  expect(msg("p2")?.labels).not.toContain("STARRED");
  expect(
    a.messages.filter((m) => m.labels.includes("SENT") && m.raw.includes("Never to be sent")),
  ).toEqual([]);
  expect((await gmail.tokens()).map((k) => k.account)).toEqual([B]);

  // Re-adding A: its Inbox matches Gmail again, and z has nothing to undo.
  await gmail.nextSignIn({ email: A });
  await command(app, "Add account");
  await expect(app.account(A)).toBeVisible();
  await app.settle();
  expect(await avatarOrder(app)).toEqual([B, A]);
  await app.press("Control+2");
  await expect(app.rows).toHaveCount(A_INBOX.length);
  expect((await app.subjects()).toSorted()).toEqual(A_INBOX);
  const modifies = (await gmail.requests("messages.batchModify", { account: A })).length;
  await app.press("z");
  await app.settle();
  await expect(app.rows).toHaveCount(A_INBOX.length);
  expect(await gmail.requests("messages.batchModify", { account: A })).toHaveLength(modifies);

  // a: remove B while A is on screen; nothing is queued.
  await app.press("Meta+,");
  await settingsBlock(app, B).getByRole("button", { name: "Remove account" }).click();
  await expect(settings.getByText(/^Remove ben@bolt\.test\?/)).toHaveText("Remove ben@bolt.test?");
  await settings.getByRole("button", { name: "Remove", exact: true }).click();
  await expect(app.account(B)).toHaveCount(0);
  await app.press("Escape");
  await app.settle();
  expect((await gmail.tokens()).map((k) => k.account)).toEqual([A]);
  expect(await gmail.revocations()).toContainEqual({ account: B, token: tokens[B] });
  await expect(app.rows).toHaveCount(A_INBOX.length);
  await expect(app.account(A)).toBeVisible();

  // c: removing the last account shows the first-run screen.
  await app.press("Meta+,");
  await settingsBlock(app, A).getByRole("button", { name: "Remove account" }).click();
  await settings.getByRole("button", { name: "Remove", exact: true }).click();
  await expect(app.firstRunButton).toBeVisible();
  await expect(app.page.getByRole("navigation")).toHaveCount(0);
});

test("ACC-08 invalid_grant shows Sign in again; the queue and outbox wait, then recover", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  const errors = await app.errorToasts.count();

  // Offline, so the archive and the reply are still waiting when Google revokes the grant.
  await gmail.offline(true);
  await expect(app.highlighted).toContainText("Person 1");
  await app.press("e");
  await expect(app.highlighted).toContainText("Person 2");
  await app.press("Enter");
  await expect(app.conversationTitle).toHaveText("Person 2");
  await app.settle();
  await app.reply("r");
  await app.type("Waiting for sign-in");
  await app.settle();
  await app.press("Meta+Enter");
  await gmail.invalidGrant(A, true);
  await gmail.expireAccessTokens(A);
  await gmail.offline(false);
  await app.advance(11_000);
  await app.settle();

  // Signed out: the marker and its tooltip, and the panel says so in place of the mail.
  await expect(app.signInMarker(A)).toBeVisible();
  await expect(app.account(A)).toHaveAttribute("title", /Sign in again/);
  await expect(app.signInMarker(B)).toHaveCount(0);
  await expect(app.page.getByText(`You're signed out of ${A}.`)).toBeVisible();
  await expect(app.page.getByRole("button", { name: "Sign in with Google" })).toBeVisible();
  await expect(app.rows).toHaveCount(0);
  await expect(app.conversationTitle).toHaveCount(0);
  await expect(app.page.getByText("Enter to sign in")).toBeVisible();
  // Mail keys do nothing there.
  await app.press("j");
  await app.press("c");
  await expect(app.editor).toHaveCount(0);
  // Nothing reached Gmail, and nothing was rejected.
  expect(await labelsOf(gmail, A, s.latest("p1").id)).toContain("INBOX");
  const sentOf = async () =>
    (await gmail.gmail(A)).messages.filter(
      (m) => m.threadId === s.thread("p2").threadId && m.labels.includes("SENT"),
    );
  expect(await sentOf()).toEqual([]);
  await expect(app.errorToasts).toHaveCount(errors);

  // Settings shows Sign in again on A's block only.
  await app.press("Meta+,");
  const settings = app.sheet("Settings");
  await expect(settings.getByRole("button", { name: "Sign in again" })).toHaveCount(1);
  await expect(settingsBlock(app, A).getByRole("button", { name: "Sign in again" })).toBeVisible();
  await app.press("Escape");

  // B keeps working.
  await app.press("Control+2");
  await expect(app.highlighted).toContainText("Bea Blue");
  await app.press("e");
  await app.settle();
  expect(await labelsOf(gmail, B, s.latest("b1").id)).not.toContain("INBOX");
  await app.press("Control+1");

  // Enter signs in again; the mail is back, and the queue and the outbox go out, once.
  await expect(app.page.getByText(`You're signed out of ${A}.`)).toBeVisible();
  await gmail.nextSignIn({ email: A });
  await gmail.invalidGrant(A, false);
  await app.press("Enter");
  await app.settle();
  await expect(app.signInMarker(A)).toHaveCount(0);
  await expect(app.page.getByText(`You're signed out of ${A}.`)).toHaveCount(0);
  await expect(app.row("Q4 plan")).toBeVisible();
  await app.press("Meta+,");
  await expect(settings.getByRole("button", { name: "Sign in again" })).toHaveCount(0);
  await app.press("Escape");
  await app.advance(10_000);
  await app.settle();
  expect(await labelsOf(gmail, A, s.latest("p1").id)).not.toContain("INBOX");
  const sent = await sentOf();
  expect(sent).toHaveLength(1);
  expect(sent[0]?.raw).toContain("Waiting for sign-in");
});

test("ACC-09 a failed sign-in adds nothing", async ({ app, gmail }) => {
  await seeds.none(gmail, app);
  await gmail.nextSignIn({ deny: true });
  await app.firstRunButton.click();
  await expect(app.page.getByText(/access_denied|denied/i)).toBeVisible();
  await expect(app.page.getByRole("navigation")).toHaveCount(0);
  expect(await gmail.tokens()).toEqual([]);

  // Abandoned in the browser: the attempt waits, and nothing is added.
  await gmail.nextSignIn({ abandon: true });
  await app.firstRunButton.click();
  await expect(app.page.getByText(/Waiting for Google in your browser/)).toBeVisible();
  await app.settle();
  await expect(app.page.getByRole("navigation")).toHaveCount(0);
  expect(await gmail.tokens()).toEqual([]);

  // A click while it waits starts over, and that attempt succeeds.
  await gmail.nextSignIn({ email: A });
  await app.firstRunButton.click();
  await expect(app.account(A)).toBeVisible();
  expect((await gmail.tokens()).map((k) => k.account)).toEqual([A]);
});

test("ACC-10 accounts with the same Gmail thread id stay separate", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  // The seed's `collide` reuses p1's thread id, but the fake numbers threads per
  // mailbox, so on B it joins b1's thread. Make a twin of a thread id A uses and B does not.
  const bThreads = new Set((await gmail.gmail(B)).messages.map((m) => m.threadId));
  const twinOf = ["addr", "replyto", "alias", "p6"].find(
    (key) => !bThreads.has(s.thread(key).threadId),
  );
  if (!twinOf) throw new Error("every candidate thread id is already used on B");
  const aThread = s.thread(twinOf);
  const twin = await gmail.deliver({
    account: B,
    from: "Cole Lide <cole@collide.test>",
    subject: "B twin",
    text: "Same thread id as one of Ana's.",
    labels: ["INBOX", "CATEGORY_PERSONAL"],
    threadId: aThread.threadId,
  });
  expect(twin.threadId).toBe(aThread.threadId);
  await app.settle();
  const aBefore = await labelsOf(gmail, A, s.latest(twinOf).id);

  await app.press("Control+2");
  await app.row("B twin").click();
  await expect(app.conversationTitle).toHaveText("B twin");
  await expect(app.messages).toHaveCount(1);
  await expect(app.messageBody(0).getByText("Same thread id as one of Ana's.")).toBeVisible();
  await app.press("e");
  await app.settle();
  expect(await labelsOf(gmail, B, twin.id)).not.toContain("INBOX");

  await app.press("Escape", "Control+1");
  await app.settle();
  expect(await labelsOf(gmail, A, s.latest(twinOf).id)).toEqual(aBefore);
  await expect(app.row(aThread.subject)).toHaveCount(1);
  await app.row(aThread.subject).click();
  await expect(app.conversationTitle).toHaveText(aThread.subject);
  await expect(app.messages).toHaveCount(1);
  await expect(app.conversation).not.toContainText("Cole Lide");
  await app.settle();
  expect(await labelsOf(gmail, A, s.latest(twinOf).id)).toContain("INBOX");
});

test("ACC-11 a task left from before Sign in again does not sign the account out again", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  // A body fetch fails and backs off.
  await gmail.fault(
    { api: "threads.get", query: { format: "full" } },
    { kind: "status", status: 500 },
  );
  const from = seeds.PERSONS[0] ?? "";
  await gmail.deliver({ account: A, from, subject: "Backing off", text: "Hi." });
  await app.settle();
  // Meanwhile the grant is revoked: the next poll signs A out.
  await gmail.invalidGrant(A, true);
  await gmail.expireAccessTokens(A);
  await gmail.poll();
  await app.settle();
  await expect(app.signInMarker(A)).toBeVisible();

  // Signing in again gives a fresh grant; the old one stays dead.
  await app.press("Meta+,");
  await gmail.nextSignIn({ email: A });
  await settingsBlock(app, A).getByRole("button", { name: "Sign in again" }).click();
  await app.settle();
  await expect(app.signInMarker(A)).toHaveCount(0);
  await app.press("Escape");
  await gmail.clearFaults();

  // The old fetch's back-off ends: it must not retry on the dead grant.
  await app.advance(60_000);
  await app.settle();
  await expect(app.signInMarker(A)).toHaveCount(0);
  await gmail.deliver({ account: A, from, subject: "Still syncing", text: "Yes." });
  await app.settle();
  await expect(app.row("Still syncing")).toHaveCount(1);
});

test("ACC-12 re-adding an account while its removal is still revoking keeps the new sign-in", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  const old = (await gmail.tokens()).find((k) => k.account === B)?.token;
  // The removal's revoke is slow to answer.
  const revoke = await gmail.hold("oauth.revoke");
  await app.press("Meta+,");
  const settings = app.sheet("Settings");
  await settingsBlock(app, B).getByRole("button", { name: "Remove account" }).click();
  await settings.getByRole("button", { name: "Remove", exact: true }).click();
  await expect(app.account(B)).toHaveCount(0);
  await app.press("Escape");
  await expect
    .poll(async () => (await gmail.requests("oauth.revoke")).some((r) => r.held))
    .toBe(true);

  // B is added again before the revoke answers.
  await gmail.nextSignIn({ email: B });
  await command(app, "Add account");
  // The sign-in gets as far as it can while the revoke is held.
  await app.settle();
  await revoke.release();
  await expect(app.account(B)).toBeVisible();
  await app.settle();

  expect(await gmail.revocations()).toEqual([{ account: B, token: old }]);
  const fresh = (await gmail.tokens()).find((k) => k.account === B)?.token;
  expect(fresh, "the new token stays in the token file").toBeDefined();
  expect(fresh).not.toBe(old);
  // The new grant works: B refreshes its token and syncs.
  await gmail.expireAccessTokens(B);
  await gmail.deliver({
    account: B,
    from: "Bea Blue <bea@blue.test>",
    subject: "After",
    text: "Hi.",
  });
  await app.settle();
  await expect(app.signInMarker(B)).toHaveCount(0);
  await app.press("Control+2");
  await expect(app.row("After")).toHaveCount(1);
});

test("ACC-13 adding an account that is already signed in replays its queue once", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  // The archive is on its way to Gmail when the account is added again.
  const modify = await gmail.hold("messages.batchModify");
  await expect(app.highlighted).toContainText("Person 1");
  await app.press("e");
  await expect
    .poll(async () => (await gmail.requests("messages.batchModify")).some((r) => r.held))
    .toBe(true);
  const lists = (await gmail.requests("threads.list", { account: A })).length;
  await gmail.nextSignIn({ email: A });
  await command(app, "Add account");
  // The new runtime is running once it lists Inbox again.
  await expect
    .poll(async () => (await gmail.requests("threads.list", { account: A })).length)
    .toBeGreaterThan(lists);

  await modify.release();
  await app.settle();
  expect(await labelsOf(gmail, A, s.latest("p1").id)).not.toContain("INBOX");
  expect(
    await gmail.requests("messages.batchModify", { account: A }),
    "the queued archive went out once",
  ).toHaveLength(1);
  await expect(app.row("Person 1")).toHaveCount(0);
});

// Accounts signed in before tokens moved from the Keychain to the token file start with none.
test("ACC-14 an account with no stored token at launch shows Sign in again", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  await app.restart(async () => {
    await gmail.forgetToken(A);
  });
  await app.settle();

  // A asks for a sign-in; B, whose token is there, does not.
  await expect(app.signInMarker(A)).toBeVisible();
  await expect(app.account(A)).toHaveAttribute("title", /Sign in again/);
  await expect(app.signInMarker(B)).toHaveCount(0);
  // The sign-in screen is in place of A's mail.
  await expect(app.page.getByText(`You're signed out of ${A}.`)).toBeVisible();
  await expect(app.rows).toHaveCount(0);

  await gmail.nextSignIn({ email: A });
  await app.page.getByRole("button", { name: "Sign in with Google" }).click();
  await app.settle();
  await expect(app.signInMarker(A)).toHaveCount(0);
  await expect(app.row("Q4 plan")).toBeVisible();
  expect((await gmail.tokens()).map((t) => t.account).toSorted()).toEqual([A, B].toSorted());
  // A syncs again: an archive reaches Gmail.
  await expect(app.highlighted).toContainText("Person 1");
  await app.press("e");
  await app.settle();
  expect(await labelsOf(gmail, A, s.latest("p1").id)).not.toContain("INBOX");
});

// With one account, its avatar did nothing on a click, so it looked broken.
test("ACC-15 clicking the account on screen opens Settings; another account switches", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  await expect(app.highlighted).toContainText("Person 1");

  await app.account(A).click();
  await expect(app.sheet("Settings")).toBeVisible();
  await expect(settingsBlock(app, A)).toBeVisible();
  await app.press("Escape");
  await expect(app.sheet("Settings")).toHaveCount(0);

  await app.account(B).click();
  await expect(app.highlighted).toContainText("Bea Blue");
  await expect(app.sheet("Settings")).toHaveCount(0);
  await app.account(B).click();
  await expect(app.sheet("Settings")).toBeVisible();
});

test("ACC-16 a removal waiting on its revoke holds up no other account", async ({ app, gmail }) => {
  await seeds.standard(gmail, app, { connect: ["A", "B", "C"] });
  const tokens = await gmail.tokens();
  const revoke = await gmail.hold("oauth.revoke");
  const held = async () => (await gmail.requests("oauth.revoke")).filter((r) => r.held).length;
  const remove = async (email: string) => {
    await app.press("Meta+,");
    const settings = app.sheet("Settings");
    await settingsBlock(app, email).getByRole("button", { name: "Remove account" }).click();
    await settings.getByRole("button", { name: "Remove", exact: true }).click();
    await expect(app.account(email)).toHaveCount(0);
    await app.press("Escape");
  };

  await remove(B);
  await expect.poll(held).toBe(1);
  // A sign-in waits for that revoke, as it must (ACC-12).
  await gmail.nextSignIn({ email: B });
  await command(app, "Add account");
  await app.settle();

  // Another removal and an action on A go ahead while B's revoke is held.
  await remove(C);
  await expect.poll(held).toBe(2);
  const modifies = (await gmail.requests("messages.batchModify")).length;
  await app.row("Person 1").click();
  await expect(app.conversationTitle).toHaveText("Person 1");
  await app.press("e");
  await expect
    .poll(async () => (await gmail.requests("messages.batchModify")).length)
    .toBeGreaterThan(modifies);

  await revoke.release(undefined, { min: 2 });
  await app.settle();
  const revoked = (await gmail.revocations()).map((r) => r.account).toSorted();
  expect(revoked).toEqual([B, C].toSorted());
  expect(tokens.map((t) => t.account)).toEqual(expect.arrayContaining([A, B, C]));
  await expect(app.account(B)).toBeVisible();
});

test("ACC-17 a first sign-in shows the whole Inbox before reading the account's drafts", async ({
  app,
  gmail,
}) => {
  test.slow();
  const s = await seeds.none(gmail, app, { open: false });
  // Drafts made in Gmail: two replies on Inbox threads, and new messages.
  // Reading them all costs more than the quota bucket holds.
  const DRAFTS = 14;
  const replies = ["p4", "plan"].map((key) => ({
    subject: `Re: ${s.thread(key).subject}`,
    to: [B],
    threadId: s.thread(key).threadId,
    inReplyTo: s.latest(key).messageId,
    text: "A reply",
  }));
  const fresh = Array.from({ length: DRAFTS - replies.length }, (_, i) => ({
    subject: `Draft ${i}`,
    to: [B],
    text: "Later",
  }));
  await Promise.all([...replies, ...fresh].map((d) => gmail.createDraft(A, d)));
  await gmail.quota(true);
  await app.open();

  await gmail.nextSignIn({ email: A });
  await app.firstRunButton.click();
  await expect(app.account(A)).toBeVisible();
  // The Inbox fills in at the production budget, its split tabs included.
  await expect.poll(() => app.splitHeader(), { timeout: 60_000 }).toBe("Important 16 · Other 7");
  // Drafts are read behind the Inbox's metadata (SPEC Sync), not ahead of it.
  const drafts = await gmail.requests("drafts.get", { account: A });
  expect(drafts.length).toBeLessThan(DRAFTS / 2);
});

// The sign-in made a new row for A but started the worker for the removed row's id, so A never synced.
test("ACC-18 an account removed while its Sign in again waits in the browser comes back and syncs", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  await gmail.invalidGrant(A, true);
  await gmail.expireAccessTokens(A);
  await gmail.poll();
  await app.settle();
  await expect(app.signInMarker(A)).toBeVisible();

  // Sign in again; the consent page stays open.
  const consent = await gmail.hold("oauth.authorize");
  await gmail.nextSignIn({ email: A });
  await app.press("Meta+,");
  const settings = app.sheet("Settings");
  await settingsBlock(app, A).getByRole("button", { name: "Sign in again" }).click();
  await expect
    .poll(async () => (await gmail.requests("oauth.authorize")).some((r) => r.held))
    .toBe(true);

  // A is removed meanwhile, then the person finishes the sign-in.
  await settingsBlock(app, A).getByRole("button", { name: "Remove account" }).click();
  await settings.getByRole("button", { name: "Remove", exact: true }).click();
  await expect(app.account(A)).toHaveCount(0);
  await app.press("Escape");
  await consent.release();
  await expect(app.account(A)).toBeVisible();
  await app.settle();

  // A is back as a new account, and it syncs.
  expect(await avatarOrder(app)).toEqual([B, A]);
  await expect(app.signInMarker(A)).toHaveCount(0);
  await gmail.deliver({
    account: A,
    from: "Carol <carol@corp.test>",
    subject: "Back again",
    text: "Hi.",
  });
  await app.settle();
  await app.press("Control+2");
  await expect(app.row("Back again")).toHaveCount(1);
  await expect(app.row("Q4 plan")).toHaveCount(1);
});
