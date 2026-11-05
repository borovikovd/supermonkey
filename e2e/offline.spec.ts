// Offline (COVERAGE 5.17, SPEC §Offline): the cached mailbox stays usable,
// actions queue in SQLite and replay in order per account when the network
// returns, and a rejection during replay rolls back only its own action.
import type { App } from "./app.ts";
import type { GmailMessage, GmailState, LoggedRequest } from "./control.ts";
import { expect, test } from "./fixtures.ts";
import * as seeds from "./seeds.ts";
import { A } from "./seeds.ts";

// ------------------------------------------------------------------ helpers

/** Moves the highlight to the row whose subject is exactly `subject`, with `j` / `k`. */
async function highlight(app: App, subject: string): Promise<void> {
  await expect(app.rows.filter({ hasText: subject }).first()).toBeVisible();
  // A view switch restores its highlight after the rows arrive.
  await app.settle();
  const { subjects, at } = await app.list.evaluate((list) => {
    const rows = Array.from(list.querySelectorAll("[data-row]"))
      .map((el) => ({ el, top: el.getBoundingClientRect().top }))
      .toSorted((a, b) => a.top - b.top);
    return {
      subjects: rows.map(
        (r) => r.el.querySelector(".flex-1 > .truncate")?.firstChild?.textContent ?? "",
      ),
      at: rows.findIndex((r) => r.el.getAttribute("aria-selected") === "true"),
    };
  });
  const to = subjects.indexOf(subject);
  if (to < 0) throw new Error(`no row "${subject}" in ${JSON.stringify(subjects)}`);
  const from = Math.max(at, 0);
  await app.press(...Array.from({ length: Math.abs(to - from) }, () => (to > from ? "j" : "k")));
  await expect(app.highlighted).toContainText(subject);
}

/** Opens a palette picker (`l`, `h`) and picks the first item matching `query`. */
async function pick(app: App, key: string, query = ""): Promise<void> {
  await app.press(key);
  await expect(app.palette).toBeVisible();
  if (query) await app.type(query);
  await expect(app.palette.locator("[cmdk-item]").first()).toBeVisible();
  await app.press("Enter");
  await expect(app.palette).toHaveCount(0);
}

const messagesOf = (state: GmailState, threadId: string): GmailMessage[] =>
  state.messages.filter((m) => m.threadId === threadId);

/** Gmail's "label on a thread": some message outside trash and spam carries it (§Views). */
const threadHas = (state: GmailState, threadId: string, label: string): boolean =>
  messagesOf(state, threadId).some(
    (m) => m.labels.includes(label) && !m.labels.includes("TRASH") && !m.labels.includes("SPAM"),
  );

const MUTATING = new Set([
  "messages.batchModify",
  "labels.create",
  "drafts.create",
  "drafts.update",
  "drafts.delete",
  "drafts.send",
  "messages.trash",
]);

/** Mutating requests Gmail answered with success, after `afterSeq`, in arrival order. */
async function applied(app: App, afterSeq: number): Promise<LoggedRequest[]> {
  const all = await app.gmail.requests();
  return all.filter(
    (r) =>
      r.seq > afterSeq &&
      MUTATING.has(r.api) &&
      typeof r.answer === "number" &&
      r.answer >= 200 &&
      r.answer < 300,
  );
}

async function lastSeq(app: App): Promise<number> {
  return (await app.gmail.requests()).at(-1)?.seq ?? -1;
}

type ModifyBody = { ids: string[]; addLabelIds: string[]; removeLabelIds: string[] };

function modifyBody(r: LoggedRequest | undefined): ModifyBody {
  const b = r?.body;
  if (typeof b !== "object" || b === null || !("ids" in b)) {
    throw new Error(`not a batchModify body: ${JSON.stringify(b)}`);
  }
  // The request log holds the JSON body the app sent to messages.batchModify.
  // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- shape checked above
  return b as ModifyBody;
}

// ------------------------------------------------------------------ cases

test("OFF-01 airplane mode: read, search, autocomplete, and act; it all lands when the network returns", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  await gmail.offline(true);

  // Read: the cached thread opens with its bodies.
  await highlight(app, "Q4 plan");
  await app.press("Enter");
  await expect(app.conversationTitle).toHaveText("Q4 plan");
  await expect(
    app.conversation.locator("iframe[data-frame]").last().contentFrame().locator("body"),
  ).toContainText("Plan message 12");
  await app.press("Escape");

  // Search: local results, with the offline note.
  await app.press("/");
  await app.type("venue");
  await app.advance(300);
  await expect(app.row("Offsite venue")).toHaveCount(1);
  await expect(app.page.getByText("Results from this Mac")).toBeVisible();
  await app.press("Escape");

  // Autocomplete from the local contacts.
  await app.compose();
  await app.type("car");
  await expect(app.suggestions("To").filter({ hasText: "carol@corp.test" })).toHaveCount(1);
  await app.press("Backspace", "Backspace", "Backspace", "Escape", "Escape");
  await expect(app.page.getByRole("heading", { name: "New Message" })).toHaveCount(0);
  await app.settle();

  // Act: archive leaves Inbox with no toast.
  const toastsBefore = await app.toasts.count();
  await highlight(app, "Person 1");
  await app.press("e");
  await expect(app.row("Person 1")).toHaveCount(0);
  await app.settle();
  await expect(app.errorToasts).toHaveCount(0);
  expect(await app.toasts.count()).toBeLessThanOrEqual(toastsBefore);

  // Send: the outbox waits; the send toast says it is delayed.
  await highlight(app, "Person 2");
  await app.reply("r");
  await app.type("Written on the plane");
  await app.settle();
  await app.press("Meta+Enter");
  await expect(app.toast("Sending")).toHaveCount(1);
  await app.advance(10_000);
  await expect(app.toast("Email delayed")).toHaveCount(1);
  const offlineA = await gmail.gmail(A);
  expect(threadHas(offlineA, s.thread("p1").threadId, "INBOX")).toBe(true);

  await gmail.offline(false);
  await app.settle();
  await expect(app.toast(/^Sent/)).toHaveCount(1);
  const a = await gmail.gmail(A);
  expect(threadHas(a, s.thread("p1").threadId, "INBOX")).toBe(false);
  const reply = messagesOf(a, s.thread("p2").threadId).find((m) => m.labels.includes("SENT"));
  expect(reply?.raw).toContain("Written on the plane");
});

test("OFF-02 every kind of queued action replays in order", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);

  // Online first: search caches archivedOld, ownAlias gets snoozed, and a
  // Gmail draft arrives to be discarded later.
  await app.press("/");
  await app.type("Archived before");
  await app.advance(300);
  await expect(app.row("Archived before")).toHaveCount(1);
  await app.press("Escape");
  await highlight(app, "Alias note");
  await pick(app, "h");
  await expect(app.row("Alias note")).toHaveCount(0);
  const spare = await gmail.createDraft(
    A,
    { subject: "Spare draft", to: ["kim@kite.test"], html: "<p>Delete me.</p>" },
    { poll: true },
  );
  await app.settle();
  const since = await lastSeq(app);

  await gmail.offline(true);
  // 1–5: archive, trash, spam, star, mark unread.
  await highlight(app, "Person 1");
  await app.press("e");
  await highlight(app, "Person 2");
  await app.press("#");
  await highlight(app, "Person 3");
  await app.press("!");
  await highlight(app, "Person 4");
  await app.press("s");
  await highlight(app, "Person 6");
  await app.press("u");
  await expect(app.row("Person 6")).toHaveAttribute("data-unread");
  // 6: return to inbox, on a search hit.
  await app.press("/");
  await app.type("Archived before");
  await expect(app.row("Archived before")).toHaveCount(1);
  await app.press("Enter");
  await highlight(app, "Archived before");
  await app.press("Shift+E");
  await app.settle();
  // Back to Inbox through the field: Esc on the results is not under test here.
  await app.press("/");
  await expect(app.page.getByPlaceholder("Search")).toBeFocused();
  await app.press("Escape");
  await expect(app.row("Archived before")).toHaveCount(1);
  // 7–8: add a label; create a label and apply it.
  await highlight(app, "Fold check");
  await pick(app, "l", "Travel");
  await highlight(app, "Guild meetup");
  await app.press("l");
  await app.type("NewL");
  await expect(app.palette.getByText('Create label "NewL"')).toBeVisible();
  await app.press("Enter");
  await expect(app.palette).toHaveCount(0);
  // 9–10: snooze; unsnooze from Snoozed.
  await highlight(app, "Budget draft");
  await pick(app, "h");
  await expect(app.row("Budget draft")).toHaveCount(0);
  await app.press("g", "h");
  await highlight(app, "Alias note");
  await app.press("Shift+E");
  await expect(app.row("Alias note")).toHaveCount(0);
  await app.press("g", "i");
  // 11: a draft upsert.
  await highlight(app, "Ticket #42");
  await app.reply("r");
  await app.type("Queued draft text");
  await app.settle();
  await app.press("Escape", "Escape");
  await expect(app.editor).toHaveCount(0);
  // 12: discard a draft that is already in Gmail.
  await app.press("Escape", "g", "d");
  await highlight(app, "Spare draft");
  await app.press("Enter");
  await expect(app.editor).toBeVisible();
  await app.settle();
  await app.press("Meta+Shift+Comma");
  await expect(app.toast("Draft discarded")).toHaveCount(1);
  await app.settle();
  expect(await applied(app, since)).toEqual([]);

  await gmail.offline(false);
  await app.settle();

  const a = await gmail.gmail(A);
  const t = (key: string) => s.thread(key).threadId;
  expect(threadHas(a, t("p1"), "INBOX")).toBe(false);
  expect(threadHas(a, t("p2"), "INBOX")).toBe(false);
  expect(messagesOf(a, t("p2")).every((m) => m.labels.includes("TRASH"))).toBe(true);
  expect(messagesOf(a, t("p3")).every((m) => m.labels.includes("SPAM"))).toBe(true);
  expect(threadHas(a, t("p3"), "INBOX")).toBe(false);
  expect(threadHas(a, t("p4"), "STARRED")).toBe(true);
  expect(threadHas(a, t("p6"), "UNREAD")).toBe(true);
  expect(threadHas(a, t("archivedOld"), "INBOX")).toBe(true);
  expect(threadHas(a, t("fold"), s.label(A, "Travel"))).toBe(true);
  const newL = a.labels.find((l) => l.name === "NewL");
  expect(newL).toBeDefined();
  expect(threadHas(a, t("alias"), newL?.id ?? "missing")).toBe(true);
  expect(threadHas(a, t("own"), "INBOX")).toBe(false);
  expect(threadHas(a, t("ownAlias"), "INBOX")).toBe(true);
  const replyDraft = a.drafts.find((d) => d.message.threadId === t("replyto"));
  expect(replyDraft?.message.raw).toContain("Queued draft text");
  expect(a.drafts.map((d) => d.id)).not.toContain(spare.id);

  // The replay follows the action order.
  const log = await applied(app, since);
  expect(log.map((r) => r.api)).toEqual([
    ...Array.from({ length: 7 }, () => "messages.batchModify"),
    "labels.create",
    ...Array.from({ length: 3 }, () => "messages.batchModify"),
    "drafts.create",
    "drafts.delete",
  ]);
  const ids = (key: string) =>
    s
      .thread(key)
      .messages.map((m) => m.id)
      .toSorted();
  const modifies = log.filter((r) => r.api === "messages.batchModify").map(modifyBody);
  expect(modifies.map((b) => b.ids.toSorted())).toEqual([
    ids("p1"),
    ids("p2"),
    ids("p3"),
    ids("p4"),
    [s.latest("p6").id],
    ids("archivedOld"),
    ids("fold"),
    ids("alias"),
    ids("own"),
    ids("ownAlias"),
  ]);
  expect(modifies[6]?.addLabelIds).toEqual([s.label(A, "Travel")]);
  expect(modifies[7]?.addLabelIds).toEqual([newL?.id]);
  expect(log.at(-1)?.path).toContain(spare.id);

  await expect(app.errorToasts).toHaveCount(0);
  await expect(app.toasts.filter({ hasNotText: "Draft discarded" })).toHaveCount(0);
});

test("OFF-03 the queue survives a restart", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  await gmail.offline(true);
  await highlight(app, "Person 1");
  await app.press("e");
  await highlight(app, "Person 2");
  await app.press("s");
  await expect(app.row("Person 2")).toHaveAttribute("data-starred");
  await app.settle();

  await app.restart();
  await expect(app.row("Person 3")).toHaveCount(1);
  await expect(app.row("Person 1")).toHaveCount(0);
  await expect(app.row("Person 2")).toHaveAttribute("data-starred");
  const offline = await gmail.gmail(A);
  expect(threadHas(offline, s.thread("p1").threadId, "INBOX")).toBe(true);
  expect(threadHas(offline, s.thread("p2").threadId, "STARRED")).toBe(false);

  await gmail.offline(false);
  await app.settle();
  const a = await gmail.gmail(A);
  expect(threadHas(a, s.thread("p1").threadId, "INBOX")).toBe(false);
  expect(threadHas(a, s.thread("p2").threadId, "STARRED")).toBe(true);
});

test("OFF-04 a queued action wins on the messages it recorded, without hiding newer mail", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  const m1 = s.message("p1", 1);
  await gmail.offline(true);
  await highlight(app, "Person 1");
  await app.press("e");
  await expect(app.row("Person 1")).toHaveCount(0);
  await app.settle();

  // Another client stars m1 and moves it to Inbox; a reply arrives. No poll yet.
  await gmail.modify(A, m1.id, ["STARRED", "INBOX"], []);
  const m2 = await gmail.deliver({
    account: A,
    from: seeds.PERSONS[0] ?? "",
    subject: "Re: Person 1",
    text: "A newer message.",
    threadId: m1.threadId,
    inReplyTo: m1.messageId,
    labels: ["INBOX", "UNREAD", "CATEGORY_PERSONAL"],
    poll: false,
  });

  await gmail.offline(false);
  await app.settle();

  const a = await gmail.gmail(A);
  const g1 = a.messages.find((m) => m.id === m1.id);
  const g2 = a.messages.find((m) => m.id === m2.id);
  expect(g1?.labels).not.toContain("INBOX");
  expect(g1?.labels).toContain("STARRED");
  expect(g2?.labels).toContain("INBOX");
  // The history sync reconciles: the thread is back for m2, starred and unread.
  const row = app.row("Person 1");
  await expect(row).toHaveCount(1);
  await expect(row).toHaveAttribute("data-starred");
  await expect(row).toHaveAttribute("data-unread");
});

test("OFF-04 a poll during the replay keeps the queued change on the messages it recorded", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  const m1 = s.message("p1", 1);
  await gmail.offline(true);
  await highlight(app, "Person 1");
  await app.press("e");
  await expect(app.row("Person 1")).toHaveCount(0);
  await app.settle();
  // Another client stars m1, so the next history carries m1 with INBOX still on it.
  await gmail.modify(A, m1.id, ["STARRED"], []);
  // The archive stays queued: Gmail keeps answering 503.
  await gmail.fault("messages.batchModify", { kind: "status", status: 503 });
  const since = await lastSeq(app);

  await gmail.offline(false);
  await app.settle();
  const after = (await gmail.requests()).filter((r) => r.seq > since);
  expect(after.some((r) => r.api === "history.list" && r.answer === 200)).toBe(true);
  expect(after.filter((r) => r.api === "messages.batchModify").map((r) => r.answer)).toContain(503);
  await expect(app.row("Person 1")).toHaveCount(0);

  await gmail.clearFaults();
  await app.settle();
  const g1 = (await gmail.gmail(A)).messages.find((m) => m.id === m1.id);
  expect(g1?.labels).not.toContain("INBOX");
  expect(g1?.labels).toContain("STARRED");
  await expect(app.row("Person 1")).toHaveCount(0);
});

test("OFF-07 a Gmail answer read before an action landed does not undo it", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  const m1 = s.message("p1", 1);
  // The next history is read at once (m1 starred, INBOX still on it) but
  // reaches the app only after the archive below has landed in Gmail.
  const history = await gmail.holdAnswer("history.list");
  await gmail.modify(A, m1.id, ["STARRED"], [], { poll: true });
  await expect
    .poll(async () => (await gmail.requests("history.list")).some((r) => r.held))
    .toBe(true);

  await highlight(app, "Person 1");
  await app.press("e");
  await app.settle();
  const before = (await gmail.gmail(A)).messages.find((m) => m.id === m1.id);
  expect(before?.labels, "the archive reached Gmail first").not.toContain("INBOX");

  await history.release();
  await app.settle();
  await expect(app.row("Person 1")).toHaveCount(0);
  await app.page.getByRole("navigation").getByRole("button", { name: /^Done/ }).click();
  const row = app.row("Person 1");
  await expect(row).toHaveCount(1);
  await expect(row, "the star from the stale answer is kept").toHaveAttribute("data-starred");
});

test("OFF-05 a rejection during replay rolls back that one action, and replay continues", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  await gmail.offline(true);
  await highlight(app, "Person 1");
  await app.press("e");
  await highlight(app, "Person 2");
  await app.press("s");
  await app.settle();
  // The fake answers with Gmail's message "Injected fault"; the reason is "Invalid label".
  await gmail.fault(
    "messages.batchModify",
    { kind: "status", status: 400, reason: "Invalid label" },
    { nth: 1 },
  );

  await gmail.offline(false);
  await app.settle();

  await expect(app.row("Person 1")).toHaveCount(1);
  await expect(app.errorToasts).toHaveCount(1);
  await expect(app.errorToasts).toContainText("Injected fault");
  const a = await gmail.gmail(A);
  expect(threadHas(a, s.thread("p1").threadId, "INBOX")).toBe(true);
  expect(threadHas(a, s.thread("p2").threadId, "STARRED")).toBe(true);

  // Nothing is left in the queue.
  await app.press("Meta+,");
  const settings = app.sheet("Settings");
  await settings.getByRole("button", { name: "Remove account" }).first().click();
  await expect(settings.getByText(`Remove ${A}?`)).toBeVisible();
  await expect(settings.getByText(/queued actions/)).toHaveCount(0);
  await settings.getByRole("button", { name: "Cancel" }).click();
});

test("OFF-06 losing the network during replay leaves the rest queued", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  const keys = await Promise.all(
    ["Person 1", "Person 2", "Person 3"].map((subject) => app.rowKey(subject)),
  );
  await gmail.offline(true);
  await highlight(app, "Person 1");
  await app.press("e", "e", "e");
  await expect(app.row("Person 3")).toHaveCount(0);
  await expect(app.highlighted).toContainText("Person 4");
  await app.settle();
  const probe = await app.watchRows();
  const since = await lastSeq(app);

  await gmail.fault("messages.batchModify", { kind: "drop" }, { nth: 2 });
  await gmail.offline(false);
  await app.settle();
  await gmail.clearFaults();
  await app.settle();

  const added = await probe.added();
  expect(added).not.toContain(keys[1]);
  expect(added).not.toContain(keys[2]);
  await expect(app.row(/Person [123]\b/)).toHaveCount(0);
  const a = await gmail.gmail(A);
  for (const key of ["p1", "p2", "p3"]) {
    expect(threadHas(a, s.thread(key).threadId, "INBOX"), key).toBe(false);
  }
  const tries = (await gmail.requests("messages.batchModify")).filter((r) => r.seq > since);
  expect(tries.filter((r) => r.answer === "dropped")).toHaveLength(1);
  const done = tries.filter((r) => r.answer === 204 || r.answer === 200).map(modifyBody);
  expect(done.map((b) => b.ids)).toEqual([
    [s.message("p1", 1).id],
    [s.message("p2", 1).id],
    [s.message("p3", 1).id],
  ]);
});

test("OFF-08 conversations opened offline read only the bodies still on screen when the network returns", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  // New threads whose bodies the background sync drops (a 400 is not retried).
  const fullRead = { api: "threads.get", query: { format: "full" } } as const;
  await gmail.fault(fullRead, { kind: "status", status: 400, reason: "invalidArgument" });
  const subjects = ["Read 1", "Read 2", "Read 3", "Read 4", "Read 5", "Read 6"];
  const delivered = await gmail.deliverMany(
    subjects.map((subject, i) => ({
      account: A,
      from: "Lee Lane <lee@lane.test>",
      subject,
      text: `Body ${i + 1}.`,
      labels: ["INBOX", "UNREAD", "CATEGORY_PERSONAL"],
    })),
  );
  await app.settle();
  await gmail.clearFaults();
  const keys = await Promise.all(subjects.map((subject) => app.rowKey(subject)));
  const gmailIds = new Map(keys.map((key, i) => [key, delivered[i]?.threadId ?? ""]));

  // Offline: every open and prefetch asks for bodies that can't be read yet.
  await gmail.offline(true);
  await app.row("Read 6").click();
  await expect(app.conversationTitle).toHaveText("Read 6");
  await ["j", "j", "j", "j", "k", "k", "k", "j", "j"].reduce(async (before, key) => {
    await before;
    await app.press(key);
    await app.settle();
  }, Promise.resolve());
  const shown = Object.keys(
    await app.page.evaluate<Record<string, string>>(`Object.fromEntries(
      Object.entries(window.__ub.useMirror.getState().subs)
        .filter(([, s]) => s.type === "conversation").map(([id, s]) => [String(s.key.thread), id]))`),
  );
  const needed = shown.flatMap((key) => gmailIds.get(key) ?? []);
  expect(needed.length).toBeGreaterThan(0);
  const since = await lastSeq(app);

  await gmail.offline(false);
  await app.settle();
  const reads = (await gmail.requests("threads.get"))
    .filter((r) => r.seq > since && r.params.format === "full")
    .map((r) => r.path.split("/").at(-1) ?? "");
  // One read per thread still on screen; none for those left behind.
  expect(reads.toSorted()).toEqual(needed.toSorted());
});
