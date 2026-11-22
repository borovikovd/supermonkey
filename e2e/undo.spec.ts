import type { Route } from "@playwright/test";

import type { App } from "./app.ts";
import type { Control, GmailState } from "./control.ts";
import { expect, test } from "./fixtures.ts";
import * as seeds from "./seeds.ts";
import { A, B, type Seeded } from "./seeds.ts";

function threadHas(state: GmailState, threadId: string, label: string): boolean {
  const msgs = state.messages.filter((m) => m.threadId === threadId && !m.labels.includes("DRAFT"));
  if (label === "TRASH" || label === "SPAM") return msgs.some((m) => m.labels.includes(label));
  return msgs.some(
    (m) => !m.labels.includes("TRASH") && !m.labels.includes("SPAM") && m.labels.includes(label),
  );
}

async function has(
  gmail: Control,
  s: Seeded,
  key: string,
  label: string,
  account = A,
): Promise<boolean> {
  return threadHas(await gmail.gmail(account), s.thread(key).threadId, label);
}

async function labelsOf(gmail: Control, s: Seeded, keys: string[]) {
  const state = await gmail.gmail(A);
  return keys.flatMap((k) =>
    s.thread(k).messages.map((d) => ({
      key: k,
      labels: (state.messages.find((m) => m.id === d.id)?.labels ?? []).toSorted(),
    })),
  );
}

async function highlight(app: App, text: string): Promise<void> {
  await expect(app.row(text)).toHaveCount(1);
  await expect(app.highlighted).toHaveCount(1);
  const texts = await app.rows.allInnerTexts();
  const target = texts.findIndex((t) => t.includes(text));
  const at = await app.rows.evaluateAll((els) =>
    els.findIndex((el) => el.getAttribute("aria-selected") === "true"),
  );
  const delta = target - at;
  const key = delta >= 0 ? "j" : "k";
  await app.press(...Array.from({ length: Math.abs(delta) }, () => key));
  await expect(app.highlighted).toContainText(text);
}

const repeat = (key: string, n: number) => Array.from({ length: n }, () => key);

async function parkSubmits(app: App): Promise<() => Promise<void>> {
  const parked: Route[] = [];
  let open = false;
  const submit = /\/invoke\/submit$/;
  await app.page.route(submit, (route) => (open ? route.continue() : parked.push(route)));
  return async () => {
    open = true;
    await Promise.all(parked.map((r) => r.continue()));
    await app.page.unroute(submit);
  };
}

test("UND-01 z walks back through the stack", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  const keys = ["p1", "p2", "p3"];
  const seeded = await labelsOf(gmail, s, keys);
  const order = await app.subjects();

  await app.press("z");
  await app.settle();
  expect(await gmail.requests("messages.batchModify")).toEqual([]);
  expect(await app.subjects()).toEqual(order);

  await expect(app.highlighted).toContainText("Person 1");
  await app.press("e");
  await expect(app.highlighted).toContainText("Person 2");
  await app.press("#");
  await expect(app.highlighted).toContainText("Person 3");
  await app.press("s");
  await expect(app.row("Person 3")).toHaveAttribute("data-starred", "true");
  await expect(app.row("Person 1")).toHaveCount(0);
  await expect(app.row("Person 2")).toHaveCount(0);

  await app.press("z");
  await expect(app.row("Person 3")).not.toHaveAttribute("data-starred");
  await expect(app.row("Person 2")).toHaveCount(0);

  await app.press("z");
  await expect
    .poll(async () => (await app.subjects()).slice(0, 2))
    .toEqual(["Person 2", "Person 3"]);
  await expect(app.row("Person 1")).toHaveCount(0);
  await app.settle();
  expect(await has(gmail, s, "p2", "TRASH")).toBe(false);
  expect(await has(gmail, s, "p2", "INBOX")).toBe(true);

  const release = await parkSubmits(app);
  await app.press("z");
  await expect.poll(() => app.subjects()).toEqual(order);
  await release();
  await app.settle();
  expect(await app.subjects()).toEqual(order);
  const requests = (await gmail.requests("messages.batchModify")).length;

  await app.press("z");
  await app.settle();
  expect(await gmail.requests("messages.batchModify")).toHaveLength(requests);
  expect(await app.subjects()).toEqual(order);
  expect(await labelsOf(gmail, s, keys)).toEqual(seeded);
});

test("UND-02 undo reverts only the labels the action changed", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  await expect(app.highlighted).toContainText("Person 1");
  await app.press("e");
  await expect(app.row("Person 1")).toHaveCount(0);
  await app.settle();
  await gmail.modify(A, s.message("p1", 1).id, ["STARRED"], [], { poll: true });
  await app.settle();
  await app.press("z");

  const p1 = app.row("Person 1");
  await expect(p1).toHaveCount(1);
  await app.settle();
  await expect(p1).toHaveAttribute("data-unread", "true");
  await expect(p1).toHaveAttribute("data-starred", "true");
  const m1 = (await gmail.gmail(A)).messages.find((m) => m.id === s.message("p1", 1).id);
  expect(m1?.labels).toEqual(expect.arrayContaining(["INBOX", "STARRED", "UNREAD"]));

  const travel = s.label(A, "Travel");
  await highlight(app, "Person 2");
  await app.press("s");
  await expect(app.row("Person 2")).toHaveAttribute("data-starred", "true");
  await app.settle();
  await gmail.modify(A, s.message("p2", 1).id, [travel], [], { poll: true });
  await app.settle();
  await app.press("z");
  await expect(app.row("Person 2")).not.toHaveAttribute("data-starred");
  await app.settle();
  const m2 = (await gmail.gmail(A)).messages.find((m) => m.id === s.message("p2", 1).id);
  expect(m2?.labels).not.toContain("STARRED");
  expect(m2?.labels).toContain(travel);
});

test("UND-03 actions that make no entry are skipped by z", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  await expect(app.highlighted).toContainText("Person 1");
  await app.press("e");
  await expect(app.row("Person 1")).toHaveCount(0);

  await highlight(app, "Person 3");
  await expect(app.row("Person 3")).toHaveAttribute("data-unread", "true");
  await app.press("Enter");
  await expect(app.conversationTitle).toHaveText("Person 3");
  await app.settle();
  await app.press("Escape");
  await expect(app.row("Person 3")).not.toHaveAttribute("data-unread");

  const settings = app.sheet("Settings");
  const remoteImages = settings.getByRole("switch", { name: "Load remote images" });
  const muteA = settings.getByRole("switch", { name: "Mute notifications" }).first();
  await app.press("Meta+,");
  await expect(remoteImages).toHaveAttribute("aria-checked", "true");
  await remoteImages.click();
  await expect(remoteImages).toHaveAttribute("aria-checked", "false");
  await app.press("Escape");
  await expect(settings).toHaveCount(0);
  await app.press("Meta+k");
  await app.type("Mute notifications");
  await expect(app.palette.getByRole("option").first()).toHaveText("Mute notifications");
  await app.press("Enter");
  await expect(app.palette).toHaveCount(0);
  await app.settle();

  await app.press("z");
  await expect(app.row("Person 1")).toHaveCount(1);
  await app.settle();
  const requests = (await gmail.requests("messages.batchModify")).length;

  await app.press("z");
  await app.settle();
  expect(await gmail.requests("messages.batchModify")).toHaveLength(requests);
  await expect(app.row("Person 1")).toHaveCount(1);
  await expect(app.row("Person 3")).not.toHaveAttribute("data-unread");
  expect(await has(gmail, s, "p1", "INBOX")).toBe(true);
  expect(await has(gmail, s, "p3", "UNREAD")).toBe(false);
  await app.press("Meta+,");
  await expect(remoteImages).toHaveAttribute("aria-checked", "false");
  await expect(settings.getByText(A, { exact: true })).toBeVisible();
  await expect(muteA).toHaveAttribute("aria-checked", "true");
});

test("UND-04 each account has its own undo history", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  await expect(app.highlighted).toContainText("Person 1");
  await app.press("e");
  await expect(app.row("Person 1")).toHaveCount(0);

  await app.press("Control+2");
  await expect(app.row("Bea Blue")).toHaveCount(1);
  const bRows = await app.subjects();
  await app.press("z");
  await app.settle();
  expect(await app.subjects()).toEqual(bRows);
  expect(await gmail.requests("messages.batchModify", { account: B })).toEqual([]);
  expect(await has(gmail, s, "p1", "INBOX")).toBe(false);

  await highlight(app, "Bea Blue");
  await app.press("e");
  await expect(app.row("Bea Blue")).toHaveCount(0);
  await app.press("z");
  await expect(app.row("Bea Blue")).toHaveCount(1);
  await app.settle();
  const bRequests = (await gmail.requests("messages.batchModify", { account: B })).length;
  await app.press("z");
  await app.settle();
  expect(await gmail.requests("messages.batchModify", { account: B })).toHaveLength(bRequests);

  await highlight(app, "Bea Blue");
  await app.press("e");
  await expect(app.row("Bea Blue")).toHaveCount(0);

  await app.press("Control+1");
  await expect(app.row("Person 2")).toHaveCount(1);
  await expect(app.row("Person 1")).toHaveCount(0);
  await app.press("z");
  await expect(app.row("Person 1")).toHaveCount(1);
  await app.settle();
  expect(await has(gmail, s, "p1", "INBOX")).toBe(true);
  expect(await has(gmail, s, "b1", "INBOX", B)).toBe(false);

  await app.press("Control+2");
  await expect(app.row("B 2")).toHaveCount(1);
  await app.press("z");
  await expect(app.row("Bea Blue")).toHaveCount(1);
  await app.settle();
  expect(await has(gmail, s, "b1", "INBOX", B)).toBe(true);
});

test("UND-05 a quick e followed by z z undoes the right entries", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  await highlight(app, "Person 6");
  await app.press("s");
  await expect(app.row("Person 6")).toHaveAttribute("data-starred", "true");
  await highlight(app, "Person 1");
  await app.press("e");
  await expect(app.row("Person 1")).toHaveCount(0);
  await app.settle();
  await expect(app.highlighted).toContainText("Person 2");

  await app.page.keyboard.type("ezz");

  await expect(app.row("Person 1")).toHaveCount(1);
  await expect(app.row("Person 2")).toHaveCount(1);
  await app.settle();
  await expect(app.row("Person 6")).toHaveAttribute("data-starred", "true");
  expect(await has(gmail, s, "p1", "INBOX")).toBe(true);
  expect(await has(gmail, s, "p2", "INBOX")).toBe(true);
  expect(await has(gmail, s, "p6", "STARRED")).toBe(true);
});

test("UND-06 z still works after a restart", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  await expect(app.highlighted).toContainText("Person 1");
  await app.press("e");
  await app.settle();
  expect(await has(gmail, s, "p1", "INBOX")).toBe(false);

  await app.restart();
  await expect(app.row("Person 2")).toHaveCount(1);
  await expect(app.row("Person 1")).toHaveCount(0);
  await app.press("z");
  await expect(app.row("Person 1")).toHaveCount(1);
  await app.settle();
  expect(await has(gmail, s, "p1", "INBOX")).toBe(true);
});

test("UND-07 only the last 50 entries are kept", async ({ app, gmail }) => {
  test.slow();
  const s = await seeds.standard(gmail, app);
  const starred = async () => {
    const state = await gmail.gmail(A);
    return ["p1", "p2"].map((k) => threadHas(state, s.thread(k).threadId, "STARRED"));
  };
  const shown = async () =>
    Promise.all(
      ["Person 1", "Person 2"].map(
        async (p) => (await app.row(p).getAttribute("data-starred")) !== null,
      ),
    );

  await expect(app.highlighted).toContainText("Person 1");
  const keys = Array.from({ length: 51 }, (_, i) =>
    i < 50 ? ["s", i % 2 === 0 ? "j" : "k"] : ["s"],
  ).flat();
  await app.press(...keys);
  await app.settle();
  expect(await shown()).toEqual([false, true]);
  expect(await starred()).toEqual([false, true]);

  await app.press(...repeat("z", 49));
  await app.settle();
  expect(await shown()).toEqual([true, true]);
  expect(await starred()).toEqual([true, true]);

  await app.press("z");
  await app.settle();
  expect(await shown()).toEqual([true, false]);
  expect(await starred()).toEqual([true, false]);
  const requests = (await gmail.requests("messages.batchModify")).length;

  await app.press("z");
  await app.settle();
  expect(await shown()).toEqual([true, false]);
  expect(await starred()).toEqual([true, false]);
  expect(await gmail.requests("messages.batchModify")).toHaveLength(requests);
});

test("UND-08 undoing a trash does not bring back a reply whose send was undone", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  const addr = s.thread("addr");
  await highlight(app, "Offsite venue");
  await app.reply("r");
  await app.type("Count me in.");
  await app.settle();
  await app.press("Meta+Enter");
  await app.advance(9_999);
  await app.advance(1);
  const sentToast = app.toast("Sent");
  await expect(sentToast).toBeVisible();
  const sentReply = () =>
    gmail
      .gmail(A)
      .then((g) =>
        g.messages.find((m) => m.threadId === addr.threadId && m.labels.includes("SENT")),
      );
  expect(await sentReply()).toBeDefined();

  await sentToast.getByRole("button", { name: "Undo" }).click();
  await app.settle();
  expect((await sentReply())?.labels).toContain("TRASH");

  await expect(app.conversationTitle).toHaveText("Offsite venue");
  await app.press("#");
  await expect(app.conversationTitle).not.toHaveText("Offsite venue");
  await app.press("z");
  await app.press("Escape");
  await expect(app.row("Offsite venue")).toHaveCount(1);
  await app.settle();

  const state = await gmail.gmail(A);
  const original = state.messages.find((m) => m.id === addr.messages[0]?.id);
  expect(original?.labels).toContain("INBOX");
  expect(original?.labels).not.toContain("TRASH");
  expect((await sentReply())?.labels).toContain("TRASH");

  await app.row("Offsite venue").click();
  await expect(app.conversationTitle).toHaveText("Offsite venue");
  await app.settle();
  await expect(app.messages).toHaveCount(1);
  await expect(app.conversation).not.toContainText("Count me in.");
});

test("UND-09 z while the action it undoes is on its way to Gmail, or queued offline, reaches Gmail after it", async ({
  app,
  gmail,
}) => {
  const s = await seeds.standard(gmail, app);
  const changes = async (id: string) =>
    (await gmail.requests("messages.batchModify"))
      .map((r) => r.body)
      .filter(
        (b): b is { ids: string[]; addLabelIds: string[]; removeLabelIds: string[] } =>
          typeof b === "object" && b !== null && "ids" in b && Array.isArray(b.ids),
      )
      .filter((b) => b.ids.includes(id))
      .map((b) => ({ add: b.addLabelIds, remove: b.removeLabelIds }));

  const p1 = s.message("p1", 1).id;
  const modify = await gmail.hold("messages.batchModify");
  await highlight(app, "Person 1");
  await app.press("e");
  await expect(app.row("Person 1")).toHaveCount(0);
  await expect
    .poll(async () => (await gmail.requests("messages.batchModify")).some((r) => r.held))
    .toBe(true);
  await app.press("z");
  await expect(app.row("Person 1")).toHaveCount(1);
  await modify.release();
  await app.settle();
  expect(await has(gmail, s, "p1", "INBOX")).toBe(true);
  expect(await changes(p1)).toEqual([
    { add: [], remove: expect.arrayContaining(["INBOX"]) },
    { add: expect.arrayContaining(["INBOX"]), remove: [] },
  ]);
  await expect(app.row("Person 1")).toHaveCount(1);

  const p2 = s.message("p2", 1).id;
  await gmail.offline(true);
  await highlight(app, "Person 2");
  await app.press("e");
  await expect(app.row("Person 2")).toHaveCount(0);
  await app.press("z");
  await expect(app.row("Person 2")).toHaveCount(1);
  await app.settle();
  await gmail.offline(false);
  await app.settle();
  expect(await has(gmail, s, "p2", "INBOX")).toBe(true);
  expect(await changes(p2)).toEqual([
    { add: [], remove: expect.arrayContaining(["INBOX"]) },
    { add: expect.arrayContaining(["INBOX"]), remove: [] },
  ]);
  await expect(app.row("Person 2")).toHaveCount(1);
  await app.expectNoErrorToasts();
});
