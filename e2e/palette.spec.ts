// The command palette: open, filter, run, close, and each command doing what
// its key does.
import type { Locator } from "@playwright/test";

import type { App } from "./app.ts";
import { expect, test } from "./fixtures.ts";
import * as seeds from "./seeds.ts";
import { A, B } from "./seeds.ts";

/** The palette's items, top to bottom. */
const items = (app: App): Locator => app.palette.locator("[cmdk-item]");

/** An item by its label, exactly. */
const item = (app: App, label: string): Locator =>
  items(app).filter({ has: app.page.getByText(label, { exact: true }) });

/** Opens the palette, types `query`, and runs the item labelled `label`. */
async function run(app: App, label: string, query = label): Promise<void> {
  await app.press("Meta+k");
  await expect(app.palette).toBeVisible();
  await app.type(query);
  await item(app, label).click();
}

/** A message's labels in Gmail. */
async function labelsOf(app: App, id: string): Promise<string[]> {
  const m = (await app.gmail.gmail(A)).messages.find((x) => x.id === id);
  if (!m) throw new Error(`no message ${id}`);
  return m.labels;
}

const viewTitle = (app: App): Locator => app.page.locator("div:has(> button[title='Search (/)'])");

test("PAL-01 open, filter, run, and close the palette", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  await app.press("Meta+k");
  await expect(app.page.getByRole("dialog", { name: "Type a command" })).toBeVisible();
  await expect(app.palette.getByRole("combobox")).toBeFocused();

  // Actions and views, each with its shortcut on the right.
  await expect(item(app, "Mark done").locator("kbd")).toHaveText(["E"]);
  await expect(item(app, "Go to Sent").locator("kbd")).toHaveText(["G", "T"]);
  await expect(item(app, "Trash").locator("kbd")).toHaveText(["#"]);
  await expect(item(app, "Go to Starred")).toHaveCount(1);

  // "sta" narrows the list to the star command and the Starred view first.
  await app.type("sta");
  await expect(items(app).first()).toBeVisible();
  const shown = await items(app).evaluateAll((els) =>
    els.map((el) => el.querySelector("span")?.textContent ?? ""),
  );
  expect(shown.slice(0, 2).toSorted()).toEqual(["Go to Starred", "Star or unstar"]);
  expect(shown).not.toContain("Mark done");
  expect(shown).not.toContain("Compose");

  // Choosing Go to Starred shows Starred and closes the palette.
  await item(app, "Go to Starred").click();
  await expect(app.palette).toHaveCount(0);
  await expect(viewTitle(app)).toHaveText("Starred");
  await expect(app.row("Trip ideas")).toHaveCount(1);

  // Esc closes it with nothing changed.
  const rows = await app.subjects();
  await app.press("Meta+k");
  await expect(app.palette).toBeVisible();
  await app.type("e");
  await app.press("Escape");
  await expect(app.palette).toHaveCount(0);
  await app.settle();
  expect(await app.subjects()).toEqual(rows);
  await expect(viewTitle(app)).toHaveText("Starred");
  expect(await gmail.requests("messages.batchModify")).toEqual([]);
});

test("PAL-02 palette commands do what their keys do", async ({ app, gmail }) => {
  test.slow();
  const s = await seeds.standard(gmail, app);
  await expect(app.highlighted).toContainText("Person 1");

  // Mark done (TRI-01): the row leaves at once, the highlight moves on, Gmail archives.
  await run(app, "Mark done");
  await expect(app.palette).toHaveCount(0);
  await expect(app.row("Person 1")).toHaveCount(0);
  await expect(app.highlighted).toContainText("Person 2");
  // Trash (TRI-03).
  await run(app, "Trash");
  await expect(app.row("Person 2")).toHaveCount(0);
  await expect(app.highlighted).toContainText("Person 3");
  // Star or unstar, then Mark read or unread (TRI-05), on p3 (unread).
  await run(app, "Star or unstar", "Star or");
  await expect(app.highlighted).toHaveAttribute("data-starred");
  await expect(app.highlighted).toHaveAttribute("data-unread");
  await run(app, "Mark read or unread", "Mark read");
  await expect(app.highlighted).not.toHaveAttribute("data-unread");
  await app.settle();
  expect(await labelsOf(app, s.latest("p1").id)).not.toContain("INBOX");
  expect(await labelsOf(app, s.latest("p2").id)).toContain("TRASH");
  const p3 = await labelsOf(app, s.latest("p3").id);
  expect(p3).toContain("STARRED");
  expect(p3).not.toContain("UNREAD");

  // Undo (UND-01): the newest entry, the read change.
  await run(app, "Undo");
  await expect(app.highlighted).toHaveAttribute("data-unread");
  await expect(app.highlighted).toHaveAttribute("data-starred");
  await app.settle();
  expect(await labelsOf(app, s.latest("p3").id)).toContain("UNREAD");

  // Reply (REP-02), in an open conversation: the reply composer, ready to type.
  await app.press("Enter");
  await expect(app.conversationTitle).toHaveText("Person 3");
  await app.settle();
  await run(app, "Reply");
  await expect(app.editor).toBeFocused();
  await app.settle();
  await app.press("Escape", "Escape");
  await expect(app.editor).toHaveCount(0);
  await app.press("Escape");
  await expect(app.list).toBeVisible();

  // Compose (NEW-01).
  await run(app, "Compose");
  await expect(app.page.getByRole("heading", { name: "New Message" })).toBeVisible();
  await expect(app.field("To")).toBeFocused();
  await app.settle();
  await app.press("Escape", "Escape");
  await expect(app.page.getByRole("heading", { name: "New Message" })).toHaveCount(0);

  // Search (SRC-01).
  await run(app, "Search");
  await expect(app.page.getByPlaceholder("Search")).toBeFocused();
  await app.press("Escape");
  await expect(app.page.getByPlaceholder("Search")).toHaveCount(0);

  // Go to Done (NAV-05).
  await run(app, "Go to Done");
  await expect(viewTitle(app)).toHaveText("Done");
  await expect(app.row("Person 1")).toHaveCount(1);

  // Keyboard shortcuts (SET-01).
  await run(app, "Keyboard shortcuts");
  await expect(app.sheet("Keyboard shortcuts")).toBeVisible();
  await app.press("Escape");
  await expect(app.sheet("Keyboard shortcuts")).toHaveCount(0);

  // Settings (SET-01).
  await run(app, "Settings");
  await expect(app.sheet("Settings")).toBeVisible();
  await app.press("Escape");
  await expect(app.sheet("Settings")).toHaveCount(0);

  // Split Inbox settings (SPL-06): the sheet of the account on screen.
  await run(app, "Split Inbox settings");
  await expect(app.sheet(`Split Inbox · ${A}`)).toBeVisible();
  await app.press("Escape");
  await expect(app.sheet(/^Split Inbox/)).toHaveCount(0);

  // Switch account (ACC-03): both accounts, with ⌃1 and ⌃2.
  await run(app, "Switch account");
  const switcher = app.page.getByRole("dialog", { name: "Switch account" });
  await expect(switcher).toBeVisible();
  await expect(items(app)).toHaveCount(2);
  await expect(items(app).nth(0)).toContainText(A);
  await expect(items(app).nth(0).locator("kbd")).toHaveText(["⌃", "1"]);
  await expect(items(app).nth(1)).toContainText(B);
  await expect(items(app).nth(1).locator("kbd")).toHaveText(["⌃", "2"]);
  await items(app).nth(1).click();
  await expect(app.palette).toHaveCount(0);
  await expect(app.highlighted).toContainText("Bea Blue");

  // Mute / Unmute notifications (NOT-04): on the account on screen.
  await run(app, "Mute notifications");
  await app.settle();
  await app.press("Meta+,");
  const muteOf = (email: string) =>
    app
      .sheet("Settings")
      .locator("div")
      .filter({ has: app.page.getByText(email, { exact: true }) })
      .filter({ has: app.page.getByRole("button", { name: "Remove account" }) })
      .last()
      .getByRole("switch", { name: "Mute notifications" });
  await expect(muteOf(B)).toHaveAttribute("aria-checked", "true");
  await expect(muteOf(A)).toHaveAttribute("aria-checked", "false");
  await app.press("Escape");
  await gmail.clearRecorded();
  await gmail.deliver({ account: B, from: "Mo Muted <mo@muted.test>", subject: "While muted" });
  await app.settle();
  expect((await gmail.platform()).notifications).toEqual([]);

  await run(app, "Unmute notifications", "Unmute");
  await app.settle();
  await app.press("Meta+,");
  await expect(muteOf(B)).toHaveAttribute("aria-checked", "false");
  await app.press("Escape");
  await gmail.deliver({ account: B, from: "Una Loud <una@loud.test>", subject: "Unmuted" });
  await app.settle();
  const { notifications } = await gmail.platform();
  expect(notifications.map((n) => n.account)).toEqual([B]);
});
