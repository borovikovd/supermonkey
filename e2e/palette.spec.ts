import type { Locator } from "@playwright/test";

import type { App } from "./app.ts";
import { expect, test } from "./fixtures.ts";
import * as seeds from "./seeds.ts";
import { A, B } from "./seeds.ts";

const items = (app: App): Locator => app.palette.locator("[cmdk-item]");

const item = (app: App, label: string): Locator =>
  items(app).filter({ has: app.page.getByText(label, { exact: true }) });

async function run(app: App, label: string, query = label): Promise<void> {
  await app.press("Meta+k");
  await expect(app.palette).toBeVisible();
  await app.type(query);
  await item(app, label).click();
}

test("PAL-01 open, filter, run, and close the palette", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  await app.press("Meta+k");
  await expect(app.page.getByRole("dialog", { name: "Type a command" })).toBeVisible();
  await expect(app.palette.getByRole("combobox")).toBeFocused();

  await expect(item(app, "Mark done").locator("kbd")).toHaveText(["E"]);
  await expect(item(app, "Go to Sent").locator("kbd")).toHaveText(["G", "T"]);
  await expect(item(app, "Trash").locator("kbd")).toHaveText(["#"]);
  await expect(item(app, "Go to Starred")).toHaveCount(1);

  await app.type("sta");
  await expect(items(app).first()).toBeVisible();
  const shown = await items(app).evaluateAll((els) =>
    els.map((el) => el.querySelector("span")?.textContent ?? ""),
  );
  expect(shown.slice(0, 2).toSorted()).toEqual(["Go to Starred", "Star or unstar"]);
  expect(shown).not.toContain("Mark done");
  expect(shown).not.toContain("Compose");

  await item(app, "Go to Starred").click();
  await expect(app.palette).toHaveCount(0);
  await expect(app.viewTitle).toHaveText("Starred");
  await expect(app.row("Trip ideas")).toHaveCount(1);

  const rows = await app.subjects();
  await app.press("Meta+k");
  await expect(app.palette).toBeVisible();
  await app.type("e");
  await app.press("Escape");
  await expect(app.palette).toHaveCount(0);
  await app.settle();
  expect(await app.subjects()).toEqual(rows);
  await expect(app.viewTitle).toHaveText("Starred");
  expect(await gmail.requests("messages.batchModify")).toEqual([]);
});

test("PAL-02 palette commands do what their keys do", async ({ app, gmail }) => {
  test.slow();
  const s = await seeds.standard(gmail, app);
  await expect(app.highlighted).toContainText("Person 1");

  await run(app, "Mark done");
  await expect(app.palette).toHaveCount(0);
  await expect(app.row("Person 1")).toHaveCount(0);
  await expect(app.highlighted).toContainText("Person 2");
  await run(app, "Trash");
  await expect(app.row("Person 2")).toHaveCount(0);
  await expect(app.highlighted).toContainText("Person 3");
  await run(app, "Star or unstar", "Star or");
  await expect(app.highlighted).toHaveAttribute("data-starred");
  await expect(app.highlighted).toHaveAttribute("data-unread");
  await run(app, "Mark read or unread", "Mark read");
  await expect(app.highlighted).not.toHaveAttribute("data-unread");
  await app.settle();
  expect(await app.gmail.labelsOf(A, s.latest("p1").id)).not.toContain("INBOX");
  expect(await app.gmail.labelsOf(A, s.latest("p2").id)).toContain("TRASH");
  const p3 = await app.gmail.labelsOf(A, s.latest("p3").id);
  expect(p3).toContain("STARRED");
  expect(p3).not.toContain("UNREAD");

  await run(app, "Undo");
  await expect(app.highlighted).toHaveAttribute("data-unread");
  await expect(app.highlighted).toHaveAttribute("data-starred");
  await app.settle();
  expect(await app.gmail.labelsOf(A, s.latest("p3").id)).toContain("UNREAD");

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

  await run(app, "Compose");
  await expect(app.page.getByRole("heading", { name: "New Message" })).toBeVisible();
  await expect(app.field("To")).toBeFocused();
  await app.settle();
  await app.press("Escape", "Escape");
  await expect(app.page.getByRole("heading", { name: "New Message" })).toHaveCount(0);

  await run(app, "Search");
  await expect(app.page.getByPlaceholder("Search")).toBeFocused();
  await app.press("Escape");
  await expect(app.page.getByPlaceholder("Search")).toHaveCount(0);

  await run(app, "Go to Done");
  await expect(app.viewTitle).toHaveText("Done");
  await expect(app.row("Person 1")).toHaveCount(1);

  await run(app, "Split Inbox settings");
  await expect(app.sheet(`Split Inbox · ${A}`)).toBeVisible();
  await app.press("Escape");
  await expect(app.sheet(/^Split Inbox/)).toHaveCount(0);

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

  await run(app, "Unmute notifications", "Unmute");
  await app.settle();
  await app.press("Meta+,");
  await expect(muteOf(B)).toHaveAttribute("aria-checked", "false");
  await app.press("Escape");
});

async function listed(app: App): Promise<string[]> {
  await app.press("Meta+k");
  await expect(app.palette).toBeVisible();
  const labels = await items(app).evaluateAll((els) =>
    els.map((el) => el.querySelector("span")?.textContent ?? ""),
  );
  await app.press("Escape");
  await expect(app.palette).toHaveCount(0);
  return labels;
}

test("PAL-03 the palette lists only the commands that work where it opened", async ({
  app,
  gmail,
}) => {
  test.slow();
  await seeds.standard(gmail, app);
  await expect(app.highlighted).toContainText("Person 1");

  let shown = await listed(app);
  for (const label of ["Mark done", "Trash", "Reply", "Select all", "Go to Starred"]) {
    expect(shown, label).toContain(label);
  }
  for (const label of ["Return to inbox", "Reply all", "Undo", "Sign in again"]) {
    expect(shown, label).not.toContain(label);
  }
  await app.press("e");
  await expect(app.highlighted).toContainText("Person 2");
  expect(await listed(app)).toContain("Undo");

  await app.press("Enter");
  await expect(app.conversationTitle).toHaveText("Person 2");
  shown = await listed(app);
  expect(shown).toContain("Reply all");
  expect(shown).not.toContain("Select all");
  expect(shown).not.toContain("Select all from here");
  await app.reply("r");
  await expect(app.conversation.getByRole("button", { name: "Mark Done (E)" })).toBeDisabled();
  await app.press("Escape", "Escape");
  await expect(app.editor).toHaveCount(0);
  await expect(app.conversation.getByRole("button", { name: "Mark Done (E)" })).toBeEnabled();
  await app.press("Escape");

  await app.press("g", "e");
  await expect(app.viewTitle).toHaveText("Done");
  await expect(app.highlighted).toHaveCount(1);
  shown = await listed(app);
  expect(shown).toContain("Return to inbox");
  expect(shown).not.toContain("Mark done");

  await app.press("g", "d");
  await expect(app.viewTitle).toHaveText("Drafts");
  shown = await listed(app);
  for (const label of ["Mark done", "Trash", "Star or unstar", "Reply", "Forward"]) {
    expect(shown, label).not.toContain(label);
  }
  expect(shown).toContain("Compose");

  await app.press("?");
  const allMail = app
    .sheet("Keyboard shortcuts")
    .locator("div")
    .filter({ has: app.page.getByText("Go to All Mail", { exact: true }) })
    .last();
  await expect(allMail.locator("kbd")).toHaveText("g a");
  await app.press("Escape");

  await app.restart(async () => {
    await gmail.forgetToken(A);
  });
  await app.settle();
  await expect(app.page.getByText(`You're signed out of ${A}.`)).toBeVisible();
  expect(await listed(app)).toEqual([
    "Sign in again",
    "Switch account",
    "Add account",
    "Mute notifications",
    "Settings",
  ]);
});

test("PAL-04 an empty Inbox offers no mail actions and hints only the palette", async ({
  app,
  gmail,
}) => {
  await seeds.empty(gmail, app);
  await expect(app.page.getByText("Inbox zero.")).toBeVisible();
  await expect(app.hintBar).toHaveText("⌘K for commands");
  const shown = await listed(app);
  for (const label of ["Mark done", "Snooze", "Trash", "Reply", "Select all", "Switch account"]) {
    expect(shown, label).not.toContain(label);
  }
  expect(shown).toContain("Compose");

  await gmail.deliver({ account: A, from: "Solo <solo@x.test>", subject: "Only one" });
  await app.settle();
  await expect(app.highlighted).toContainText("Only one");
  await expect(app.hintBar).toHaveText(/E to Mark Done\s*·\s*H to snooze\s*·\s*⌘K for commands/);
});
