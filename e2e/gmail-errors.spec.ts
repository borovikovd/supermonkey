// Gmail errors (SPEC §Gmail errors): each outcome of a Gmail
// call, for mail actions, `drafts.create`, and body fetches. Only a rejection
// rolls an action back and shows a toast; transient failures retry silently.
import type { App } from "./app.ts";
import type { GmailMessage, GmailState, LoggedRequest, Outcome } from "./control.ts";
import { expect, test } from "./fixtures.ts";
import * as seeds from "./seeds.ts";
import { A } from "./seeds.ts";

declare global {
  interface Window {
    /** Texts of error toasts, in the order they appeared (set by `errorToastProbe`). */
    e2eErrorToasts?: string[];
  }
}

// ------------------------------------------------------------------ helpers

/** Runs steps one after another, each waiting for the one before. */
async function inSeries<T>(items: T[], step: (item: T, i: number) => Promise<void>): Promise<void> {
  await items.reduce<Promise<void>>(
    (prev, item, i) => prev.then(() => step(item, i)),
    Promise.resolve(),
  );
}

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

/**
 * Records every error toast as it appears, so a test can count them even
 * after Sonner has dismissed the older ones.
 */
async function errorToastProbe(app: App): Promise<() => Promise<string[]>> {
  await app.page.evaluate(() => {
    const seen: string[] = [];
    window.e2eErrorToasts = seen;
    // Each toast once: when it is added, or when it turns into an error toast.
    const record = (el: Element) => {
      if (el.matches('[data-sonner-toast][data-type="error"]:not([data-e2e-seen])')) {
        el.setAttribute("data-e2e-seen", "");
        seen.push(el.textContent);
      }
    };
    new MutationObserver((records) => {
      for (const r of records) {
        if (r.target instanceof Element) record(r.target);
        for (const node of Array.from(r.addedNodes)) {
          if (!(node instanceof Element)) continue;
          record(node);
          node.querySelectorAll("[data-sonner-toast]").forEach(record);
        }
      }
    }).observe(document.body, {
      childList: true,
      subtree: true,
      attributes: true,
      attributeFilter: ["data-type"],
    });
  });
  return () => app.page.evaluate(() => window.e2eErrorToasts ?? []);
}

const messagesOf = (state: GmailState, threadId: string): GmailMessage[] =>
  state.messages.filter((m) => m.threadId === threadId);

/** Gmail's "label on a thread": some message outside trash and spam carries it (§Views). */
const threadHas = (state: GmailState, threadId: string, label: string): boolean =>
  messagesOf(state, threadId).some(
    (m) => m.labels.includes(label) && !m.labels.includes("TRASH") && !m.labels.includes("SPAM"),
  );

type ModifyBody = { ids: string[]; addLabelIds: string[]; removeLabelIds: string[] };

function modifyBody(r: LoggedRequest): ModifyBody {
  const b = r.body;
  if (typeof b !== "object" || b === null || !("ids" in b)) {
    throw new Error(`not a batchModify body: ${JSON.stringify(b)}`);
  }
  // The request log holds the JSON body the app sent to messages.batchModify.
  // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- shape checked above
  return b as ModifyBody;
}

/** `batchModify` requests whose ids are exactly `ids` (in any order). */
async function modifiesOf(app: App, ids: string[]): Promise<LoggedRequest[]> {
  const want = ids.toSorted().join(",");
  return (await app.gmail.requests("messages.batchModify")).filter(
    (r) => modifyBody(r).ids.toSorted().join(",") === want,
  );
}

const byId = (x: string, y: string) => x.localeCompare(y);

const ok = (r: LoggedRequest) => typeof r.answer === "number" && r.answer >= 200 && r.answer < 300;

/** The Gmail message a fault answers with: the fake cannot set a custom one. */
const GMAIL_MESSAGE = "Injected fault";

// ------------------------------------------------------------------ cases

test("ERR-01 transient failures retry silently and keep the row as the user left it", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  const outcomes: [string, Outcome][] = [
    ["429", { kind: "status", status: 429 }],
    ["500", { kind: "status", status: 500 }],
    ["503", { kind: "status", status: 503 }],
    ["403 rateLimitExceeded", { kind: "status", status: 403, reason: "rateLimitExceeded" }],
    ["403 userRateLimitExceeded", { kind: "status", status: 403, reason: "userRateLimitExceeded" }],
    ["403 backendError", { kind: "status", status: 403, reason: "backendError" }],
    ["noResponse", { kind: "noResponse" }],
  ];
  const keys = ["p1", "p2", "p3", "p4", "p5", "p6", "plan"];
  const probe = await app.watchRows();
  const errors = await errorToastProbe(app);

  await inSeries(outcomes, async ([name, outcome], i) => {
    const key = keys[i] ?? "";
    const thread = s.thread(key);
    await highlight(app, thread.subject);
    const rowKey = await app.rowKey(thread.subject);
    await gmail.fault("messages.batchModify", outcome, { times: 2 });
    await app.press("e");
    await expect(app.row(thread.subject), name).toHaveCount(0);
    await app.advance(10_000, { stepwise: true });
    await app.settle();

    expect(await probe.added(), `${name}: the row came back`).not.toContain(rowKey);
    await expect(app.row(thread.subject), name).toHaveCount(0);
    const a = await gmail.gmail(A);
    expect(threadHas(a, thread.threadId, "INBOX"), name).toBe(false);
    const tries = await modifiesOf(
      app,
      thread.messages.map((m) => m.id),
    );
    expect(
      tries.map((r) => ok(r)),
      name,
    ).toEqual([false, false, true]);
  });

  await app.settle();
  expect(await errors()).toEqual([]);
  await expect(app.toasts).toHaveCount(0);
});

test("ERR-02 back-off timing and Retry-After", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);

  // Retry-After wins over the back-off.
  await gmail.fault(
    "messages.batchModify",
    { kind: "status", status: 429, retryAfterSecs: 7 },
    { times: 1 },
  );
  await app.press("e");
  await app.settle();
  const p1 = [s.message("p1", 1).id];
  expect(await modifiesOf(app, p1)).toHaveLength(1);
  await app.advance(6_900);
  expect(await modifiesOf(app, p1)).toHaveLength(1);
  await app.advance(100);
  const retried = await modifiesOf(app, p1);
  expect(retried.map((r) => r.answer)).toEqual([429, 204]);
  expect((retried[1]?.at ?? 0) - (retried[0]?.at ?? 0)).toBe(7_000);

  // 1 s doubling, capped at 60 s, with jitter.
  await gmail.fault("messages.batchModify", { kind: "status", status: 500 }, { times: 8 });
  await app.press("e");
  await app.settle();
  await app.advance(240_000, { stepwise: true });
  const tries = await modifiesOf(app, [s.message("p2", 1).id]);
  expect(tries.map((r) => r.answer)).toEqual([...Array.from({ length: 8 }, () => 500), 204]);
  const gaps = tries.slice(1).map((r, i) => r.at - (tries[i]?.at ?? 0));
  for (const [i, gap] of gaps.entries()) {
    const base = Math.min(1_000 * 2 ** i, 60_000);
    expect(gap, `gap ${i}`).toBeGreaterThanOrEqual(0.8 * base - 50);
    expect(gap, `gap ${i}`).toBeLessThanOrEqual(Math.min(1.2 * 1_000 * 2 ** i, 60_000) + 50);
  }
  await expect(app.errorToasts).toHaveCount(0);
});

test("ERR-03 a quiet sync status after 30 s of retrying", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  await gmail.fault("messages.batchModify", { kind: "status", status: 503 });
  await app.press("e");
  await app.settle();

  await app.advance(29_000);
  await expect(app.syncMarker(A)).toHaveCount(0);
  await expect(app.account(A)).not.toHaveAttribute("title", /Can't reach Gmail/);

  await app.advance(2_000);
  await expect(app.syncMarker(A)).toBeVisible();
  await expect(app.account(A)).toHaveAttribute("title", /· Can't reach Gmail$/);
  // Cached mail still opens.
  await highlight(app, "Ticket #42");
  await app.press("Enter");
  await expect(app.conversationTitle).toHaveText("Ticket #42");
  await expect(
    app.conversation.locator("iframe[data-frame]").last().contentFrame().locator("body"),
  ).toContainText("Your ticket is open.");
  await app.press("Escape");

  await gmail.clearFaults();
  await app.settle();
  await expect(app.syncMarker(A)).toHaveCount(0);
  await expect(app.account(A)).not.toHaveAttribute("title", /Can't reach Gmail/);
  const a = await gmail.gmail(A);
  expect(threadHas(a, s.thread("p1").threadId, "INBOX")).toBe(false);
  await expect(app.errorToasts).toHaveCount(0);
});

test("ERR-04 a rejection rolls the action back and shows Gmail's error", async ({ app, gmail }) => {
  test.slow();
  const s = await seeds.standard(gmail, app);
  const errors = await errorToastProbe(app);
  const reject = () =>
    gmail.fault(
      "messages.batchModify",
      { kind: "status", status: 400, reason: "badRequest" },
      { nth: 1 },
    );
  let toastsSoFar = 0;
  const expectNewErrorToast = async (what: string) => {
    toastsSoFar += 1;
    await expect.soft.poll(errors, { message: what, timeout: 2_000 }).toHaveLength(toastsSoFar);
    expect.soft((await errors()).at(-1), what).toContain(GMAIL_MESSAGE);
  };
  const t = (key: string) => s.thread(key).threadId;

  // e on p1, with another action right behind it that must not be affected.
  await reject();
  await highlight(app, "Person 1");
  await app.press("e");
  await highlight(app, "Fold check");
  await app.press("s");
  await app.settle();
  await expectNewErrorToast("e");
  const sections = await app.sections();
  expect(sections[0]?.subjects[0]).toBe("Person 1");
  await expect(app.row("Fold check")).toHaveAttribute("data-starred");
  let a = await gmail.gmail(A);
  expect(threadHas(a, t("p1"), "INBOX")).toBe(true);
  expect(threadHas(a, t("fold"), "STARRED")).toBe(true);

  // # on p2: back in Inbox, not in Trash.
  await reject();
  await highlight(app, "Person 2");
  await app.press("#");
  await app.settle();
  await expectNewErrorToast("#");
  await expect(app.row("Person 2")).toHaveCount(1);
  await app.press("g", "#");
  await app.settle();
  await expect(app.row("Person 2")).toHaveCount(0);
  await app.press("g", "i");

  // ! on p3: back in Inbox.
  await reject();
  await highlight(app, "Person 3");
  await app.press("!");
  await app.settle();
  await expectNewErrorToast("!");
  await expect(app.row("Person 3")).toHaveCount(1);

  // s on p4: not starred.
  await reject();
  await highlight(app, "Person 4");
  await app.press("s");
  await app.settle();
  await expectNewErrorToast("s");
  await expect(app.row("Person 4")).not.toHaveAttribute("data-starred");

  // u on p6: read again.
  await reject();
  await highlight(app, "Person 6");
  await app.press("u");
  await app.settle();
  await expectNewErrorToast("u");
  await expect(app.row("Person 6")).not.toHaveAttribute("data-unread");

  // l Travel on p5: the label is gone again.
  await reject();
  await highlight(app, "Person 5");
  await pick(app, "l", "Travel");
  await app.settle();
  await expectNewErrorToast("l");
  await app.press("l");
  await expect(app.palette.locator("[cmdk-item]").filter({ hasText: "Travel" })).toHaveText(
    "Travel",
  );
  await app.press("Escape");

  // h on own: in Inbox, with no snooze.
  await reject();
  await highlight(app, "Budget draft");
  await pick(app, "h");
  await app.settle();
  await expectNewErrorToast("h");
  await expect(app.row("Budget draft")).toHaveCount(1);
  await app.press("g", "h");
  await app.settle();
  await expect(app.rows).toHaveCount(0);
  await app.press("g", "i");

  // Shift+E on p1 in Done: back in Done.
  await highlight(app, "Person 1");
  await app.press("e");
  await app.settle();
  await app.press("g", "e");
  await reject();
  await highlight(app, "Person 1");
  await app.press("Shift+E");
  await app.settle();
  await expectNewErrorToast("Shift+E");
  await expect(app.row("Person 1")).toHaveCount(1);
  await app.press("g", "i");
  await expect(app.row("Person 1")).toHaveCount(0);

  // A 403 that is not a rate limit is a rejection too.
  await highlight(app, "Person 2");
  await gmail.fault(
    "messages.batchModify",
    { kind: "status", status: 403, reason: "insufficientPermissions" },
    { nth: 1 },
  );
  await app.press("e");
  await app.settle();
  await expectNewErrorToast("403 insufficientPermissions");
  await expect(app.row("Person 2")).toHaveCount(1);

  a = await gmail.gmail(A);
  expect(threadHas(a, t("p2"), "INBOX")).toBe(true);
  expect(threadHas(a, t("p2"), "TRASH")).toBe(false);
  expect(threadHas(a, t("p3"), "SPAM")).toBe(false);
  expect(threadHas(a, t("p4"), "STARRED")).toBe(false);
  expect(threadHas(a, t("p6"), "UNREAD")).toBe(false);
  expect(threadHas(a, t("p5"), s.label(A, "Travel"))).toBe(false);
  expect(threadHas(a, t("own"), "INBOX")).toBe(true);
});

test("ERR-05 an expired token is refreshed once, and the request is retried", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  const tokens = async () => (await gmail.requests("oauth.token", { account: A })).length;
  const before = await tokens();
  await gmail.expireAccessTokens(A);
  await app.press("e");
  await app.settle();

  expect(await tokens()).toBe(before + 1);
  const tries = await modifiesOf(app, [s.message("p1", 1).id]);
  expect(tries.map((r) => r.answer)).toEqual([401, 204]);
  const a = await gmail.gmail(A);
  expect(threadHas(a, s.thread("p1").threadId, "INBOX")).toBe(false);
  await expect(app.toasts).toHaveCount(0);
  await expect(app.signInMarker(A)).toHaveCount(0);
  await expect(app.account(A)).not.toHaveAttribute("title", /Sign in again/);
});

test("ERR-06 a 404 on part of an action drops only the missing messages", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  const errors = await errorToastProbe(app);
  const deliverThread = async (subject: string, n: number) => {
    const first = await gmail.deliver({
      account: A,
      from: "Tess Three <tess@three.test>",
      subject,
      text: `${subject} 1.`,
      labels: ["INBOX", "CATEGORY_PERSONAL"],
      poll: false,
    });
    const rest: (typeof first)[] = [];
    await inSeries(
      Array.from({ length: n - 1 }, (_, i) => i + 2),
      async (k) => {
        const prev = rest.at(-1) ?? first;
        rest.push(
          await gmail.deliver({
            account: A,
            from: "Tess Three <tess@three.test>",
            subject: `Re: ${subject}`,
            text: `${subject} ${k}.`,
            labels: ["INBOX", "CATEGORY_PERSONAL"],
            threadId: first.threadId,
            inReplyTo: prev.messageId,
            poll: false,
          }),
        );
      },
    );
    return [first, ...rest];
  };

  // Part of the thread is gone.
  const [m1, m2, m3] = await deliverThread("Three parts", 3);
  if (!m1 || !m2 || !m3) throw new Error("thread not delivered");
  await gmail.poll();
  await app.settle();
  await gmail.deleteMessage(A, m2.id);
  await highlight(app, "Three parts");
  await app.press("e");
  await app.settle();

  const all = await modifiesOf(app, [m1.id, m2.id, m3.id]);
  expect(all.map((r) => r.answer)).toEqual([404]);
  const checks = (await gmail.requests("messages.get")).filter(
    (r) => r.params.format === "minimal" && r.seq > (all[0]?.seq ?? 0),
  );
  expect(checks.map((r) => r.path.split("/").at(-1) ?? "").toSorted(byId)).toEqual(
    [m1.id, m2.id, m3.id].toSorted(byId),
  );
  const retry = await modifiesOf(app, [m1.id, m3.id]);
  expect(retry.map((r) => r.answer)).toEqual([204]);
  const a = await gmail.gmail(A);
  expect(a.messages.find((m) => m.id === m1.id)?.labels).not.toContain("INBOX");
  expect(a.messages.find((m) => m.id === m3.id)?.labels).not.toContain("INBOX");
  await expect(app.row("Three parts")).toHaveCount(0);

  // The whole thread is gone: the action finishes, and the next poll drops it.
  const [n1, n2] = await deliverThread("Two parts", 2);
  if (!n1 || !n2) throw new Error("thread not delivered");
  await gmail.poll();
  await app.settle();
  await gmail.deleteThread(A, n1.threadId);
  await highlight(app, "Two parts");
  await app.press("e");
  await app.settle();
  await expect(app.row("Two parts")).toHaveCount(0);
  expect((await modifiesOf(app, [n1.id, n2.id])).map((r) => r.answer)).toEqual([404]);
  await gmail.poll();
  await app.settle();
  await app.press("g", "e");
  await app.settle();
  await expect(app.row("Two parts")).toHaveCount(0);
  await expect(app.row("Three parts")).toHaveCount(1);
  expect(await errors()).toEqual([]);
});

test("ERR-07 drafts.create with no clear answer never makes a duplicate draft", async ({
  app,
  gmail,
}) => {
  test.slow();
  const s = await seeds.standard(gmail, app);
  const draftsOn = async (threadId: string) =>
    (await gmail.gmail(A)).drafts.filter((d) => d.message.threadId === threadId);

  /** Opens a reply on `subject`, types twice with an autosave after each, then closes it. */
  const open = async (subject: string) => {
    await highlight(app, subject);
    await app.press("Enter");
    await expect(app.conversationTitle).toHaveText(subject);
    await app.settle();
  };
  const write = async (subject: string, between?: () => Promise<void>) => {
    await open(subject);
    await app.reply("r");
    await app.type("First words");
    await app.settle();
    await between?.();
    await app.type(" and more words");
    await app.settle();
    await app.press("Escape", "Escape");
    await expect(app.editor).toHaveCount(0);
    await app.press("Escape");
    await app.settle();
  };

  const expectOneDraft = async (key: string) => {
    const drafts = await draftsOn(s.thread(key).threadId);
    expect(drafts, key).toHaveLength(1);
    expect(drafts[0]?.message.headers["x-ubermensch-draft"], key).toBeTruthy();
    expect(drafts[0]?.message.raw, key).toContain("First words and more words");
    return drafts[0]?.id;
  };

  // noResponse once: Gmail made it; the recovery finds it by its header, and edits update it.
  await gmail.fault("drafts.create", { kind: "noResponse" }, { times: 1 });
  let firstId: string | undefined;
  await write("Person 3", async () => {
    const drafts = await draftsOn(s.thread("p3").threadId);
    expect(drafts).toHaveLength(1);
    firstId = drafts[0]?.id;
  });
  expect(await expectOneDraft("p3")).toBe(firstId);
  expect(
    (await gmail.requests("drafts.create")).filter((r) => r.answer !== "noResponse"),
  ).toHaveLength(0);

  // drop once: not created; it is created on retry.
  await gmail.fault("drafts.create", { kind: "drop" }, { times: 1 });
  await write("Person 4");
  await expectOneDraft("p4");

  // statusAfterApply(500) once: Gmail made it, then answered 500.
  await gmail.fault("drafts.create", { kind: "statusAfterApply", status: 500 }, { times: 1 });
  await write("Person 5");
  await expectOneDraft("p5");

  // Two copies exist in Gmail: the app keeps one and deletes the other.
  const create = await gmail.hold("drafts.create");
  const list = await gmail.hold("drafts.list");
  await open("Person 6");
  await app.reply("r");
  await app.type("First words");
  await app.settle();
  await create.release({ kind: "noResponse" });
  // The app now lists drafts to recover: a second copy appears first.
  await expect
    .poll(async () => (await gmail.requests("drafts.list")).some((r) => r.held))
    .toBe(true);
  const [copy] = await draftsOn(s.thread("p6").threadId);
  const localId = copy?.message.headers["x-ubermensch-draft"] ?? "";
  expect(localId).not.toBe("");
  await gmail.createDraft(A, {
    to: [seeds.PERSONS[5] ?? ""],
    subject: "Re: Person 6",
    threadId: s.thread("p6").threadId,
    inReplyTo: s.latest("p6").messageId,
    headers: [["X-Ubermensch-Draft", localId]],
    html: "<p>First words</p>",
  });
  expect(await draftsOn(s.thread("p6").threadId)).toHaveLength(2);
  await list.release();
  await app.settle();
  await app.type(" and more words");
  await app.settle();
  await app.press("Escape", "Escape");
  await app.press("Escape");
  await app.settle();
  await expectOneDraft("p6");

  // Drafts lists one row per draft.
  await app.press("g", "d");
  await app.settle();
  await inSeries(["Person 3", "Person 4", "Person 5", "Person 6"], async (subject) => {
    await expect(app.row(subject), subject).toHaveCount(1);
  });
  await expect(app.rows).toHaveCount(4);
});

test("ERR-08 a 404 while fetching a body is dropped silently", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  const errors = await errorToastProbe(app);
  const hold = await gmail.hold({ api: "threads.get", query: { format: "full" } });
  const t = await gmail.deliver({
    account: A,
    from: "Gone Soon <gone@soon.test>",
    subject: "Soon gone",
    text: "This will be deleted.",
    labels: ["INBOX", "CATEGORY_PERSONAL"],
  });
  await app.settle();
  await expect(app.row("Soon gone")).toHaveCount(1);
  await gmail.deleteThread(A, t.threadId);
  await hold.release();
  await app.settle();

  await expect(app.row("Soon gone")).toHaveCount(0);
  const fetches = (await gmail.requests("threads.get")).filter((r) => r.path.endsWith(t.threadId));
  expect(fetches.map((r) => r.answer)).toContain(404);
  expect(await errors()).toEqual([]);
  // The app stays usable: another thread opens with its body.
  await highlight(app, "Person 2");
  await app.press("Enter");
  await expect(
    app.conversation.locator("iframe[data-frame]").last().contentFrame().locator("body"),
  ).toContainText("Hello from Quin.");
});

test("ERR-09 a rejected undo or label create", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  const errors = await errorToastProbe(app);

  // A rejected label create: the label and its use disappear.
  await gmail.fault("labels.create", { kind: "status", status: 400, reason: "badRequest" });
  await highlight(app, "Person 2");
  await app.press("l");
  await app.type("Bad");
  await expect(app.palette.getByText('Create label "Bad"')).toBeVisible();
  await app.press("Enter");
  await expect(app.palette).toHaveCount(0);
  await app.settle();
  await expect.soft.poll(errors).toHaveLength(1);
  expect.soft((await errors()).at(0)).toContain(GMAIL_MESSAGE);
  await app.press("l");
  await app.type("Bad");
  await expect(app.palette.locator("[cmdk-item]")).toHaveText(['Create label "Bad"']);
  await app.press("Escape");
  let a = await gmail.gmail(A);
  expect(a.labels.map((l) => l.name)).not.toContain("Bad");
  expect(messagesOf(a, s.thread("p2").threadId).flatMap((m) => m.labels)).toEqual(
    s.thread("p2").messages.flatMap(() => ["INBOX", "CATEGORY_PERSONAL"]),
  );

  // A rejected undo: p1 returns to archived.
  const before = (await errors()).length;
  await highlight(app, "Person 1");
  await app.press("e");
  await app.settle();
  await expect(app.row("Person 1")).toHaveCount(0);
  await gmail.fault(
    "messages.batchModify",
    { kind: "status", status: 400, reason: "badRequest" },
    { nth: 1 },
  );
  await app.press("z");
  await app.settle();
  a = await gmail.gmail(A);
  expect(threadHas(a, s.thread("p1").threadId, "INBOX")).toBe(false);
  await expect.soft.poll(errors).toHaveLength(before + 1);
  await expect(app.row("Person 1")).toHaveCount(0);
});

test("ERR-10 the quiet sync status clears once Gmail answers after a relaunch", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  await gmail.fault("messages.batchModify", { kind: "status", status: 503 });
  await app.press("e");
  await app.settle();
  await app.advance(31_000);
  await expect(app.syncMarker(A)).toBeVisible();

  // Gmail recovers while the app is closed.
  await app.restart(async () => {
    await gmail.clearFaults();
  });
  await app.settle();
  await expect(app.syncMarker(A)).toHaveCount(0);
  await expect(app.account(A)).not.toHaveAttribute("title", /Can't reach Gmail/);
});
