// The Settings sheet and the remote images setting (COVERAGE 5.21).
import type { Locator } from "@playwright/test";

import type { App } from "./app.ts";
import { expect, test } from "./fixtures.ts";
import { pixelDataUrl } from "./files.ts";
import * as seeds from "./seeds.ts";
import { A, B } from "./seeds.ts";

/** An account's block in the Settings sheet. */
function settingsBlock(app: App, email: string): Locator {
  return app
    .sheet("Settings")
    .locator("div")
    .filter({ has: app.page.getByText(email, { exact: true }) })
    .filter({ has: app.page.getByRole("button", { name: "Remove account" }) })
    .last();
}

const remoteImages = (app: App): Locator =>
  app.sheet("Settings").getByRole("switch", { name: "Load remote images" });

/** Paths fetched from the remote image host so far. */
async function imageHits(app: App): Promise<string[]> {
  return (await app.gmail.hits()).map((h) => h.path);
}

/** Whether an image in the open message's frame has loaded. */
function loaded(app: App, alt: string): Promise<boolean> {
  return app.conversation
    .locator("iframe[data-frame]")
    .first()
    .contentFrame()
    .locator(`img[alt="${alt}"]`)
    .evaluate((img) => img instanceof HTMLImageElement && img.complete && img.naturalWidth > 0);
}

/** The open message frame's Content-Security-Policy. */
function frameCsp(app: App): Promise<string> {
  return app.conversation
    .locator("iframe[data-frame]")
    .first()
    .evaluate((f) => {
      const doc = f instanceof HTMLIFrameElement ? f.srcdoc : "";
      return /Content-Security-Policy" content="([^"]*)"/.exec(doc)?.[1] ?? "";
    });
}

test("SET-01 what the Settings sheet holds, and how it opens", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  const settings = app.sheet("Settings");

  // Cmd+, opens it.
  await app.press("Meta+,");
  await expect(settings).toBeVisible();
  await expect(remoteImages(app)).toHaveAttribute("aria-checked", "true");
  // One block per account.
  await Promise.all(
    [A, B].map(async (email) => {
      const block = settingsBlock(app, email);
      await expect(block).toBeVisible();
      await expect(block.getByRole("switch", { name: "Mute notifications" })).toHaveAttribute(
        "aria-checked",
        "false",
      );
      await expect(block.getByRole("button", { name: "Split Inbox settings" })).toBeVisible();
      await expect(block.getByRole("button", { name: "Remove account" })).toBeVisible();
    }),
  );
  await expect(settings.getByRole("switch", { name: "Mute notifications" })).toHaveCount(2);
  // No account needs it.
  await expect(settings.getByRole("button", { name: "Sign in again" })).toHaveCount(0);
  await app.press("Escape");
  await expect(settings).toHaveCount(0);

  // The palette's Settings opens the same sheet.
  await app.press("Meta+k");
  await app.type("Settings");
  await app.palette.locator("[cmdk-item]", { hasText: /^Settings/ }).click();
  await expect(app.palette).toHaveCount(0);
  await expect(settings).toBeVisible();
  await expect(remoteImages(app)).toBeVisible();
  await expect(settings.getByRole("button", { name: "Remove account" })).toHaveCount(2);

  // Split Inbox settings opens that account's split sheet.
  await settingsBlock(app, B).getByRole("button", { name: "Split Inbox settings" }).click();
  await expect(app.sheet(`Split Inbox · ${B}`)).toBeVisible();
  await app.press("Escape");
  await expect(app.sheet(/^Split Inbox/)).toHaveCount(0);

  // ? opens the shortcut sheet; Esc closes it.
  await app.press("?");
  const shortcuts = app.sheet("Keyboard shortcuts");
  await expect(shortcuts).toBeVisible();
  await expect(shortcuts).toContainText("Mark done");
  await app.press("Escape");
  await expect(shortcuts).toHaveCount(0);
});

test("SET-02 remote images", async ({ app, gmail }) => {
  const s = await seeds.standard(gmail, app);
  // An undo entry made before the setting changes.
  await expect(app.highlighted).toContainText("Person 1");
  await app.press("e");
  await expect(app.row("Person 1")).toHaveCount(0);

  // On (the default): the remote <img> and the CSS background are fetched; data: shows.
  await app.row("Links").click();
  await expect(app.conversationTitle).toHaveText("Links");
  await expect
    .poll(() => imageHits(app))
    .toEqual(expect.arrayContaining(["/img/pixel.png", "/img/bg.png"]));
  await expect.poll(() => loaded(app, "remote pixel")).toBe(true);
  await expect.poll(() => loaded(app, "data pixel")).toBe(true);
  expect(await frameCsp(app)).not.toContain("img-src");

  // Off, with links open: it re-renders at once, with remote images blocked.
  await app.press("Meta+,");
  await remoteImages(app).click();
  await expect(remoteImages(app)).toHaveAttribute("aria-checked", "false");
  await app.press("Escape");
  await expect(app.conversationTitle).toHaveText("Links");
  await expect.poll(() => frameCsp(app)).toContain("img-src data:");
  await expect.poll(() => loaded(app, "remote pixel")).toBe(false);
  await expect.poll(() => loaded(app, "data pixel")).toBe(true);

  // Not an undo entry: z undoes the archive before it, and the setting stays off.
  await app.press("Escape");
  await app.press("z");
  await expect(app.row("Person 1")).toHaveCount(1);
  await app.settle();
  const p1 = (await gmail.gmail(A)).messages.find((m) => m.id === s.latest("p1").id);
  expect(p1?.labels).toContain("INBOX");
  await app.press("Meta+,");
  await expect(remoteImages(app)).toHaveAttribute("aria-checked", "false");
  await app.press("Escape");

  // It survives a restart.
  await app.restart();
  await app.press("Meta+,");
  await expect(remoteImages(app)).toHaveAttribute("aria-checked", "false");
  await app.press("Escape");

  // Off: a fresh message's remote images are not fetched; its data: image shows.
  await gmail.deliver({
    account: A,
    from: "Ivo Image <ivo@image.test>",
    subject: "Fresh images",
    html:
      `<p><img src="${gmail.url}/img/fresh.png" alt="remote pixel"></p>` +
      `<div style="background-image:url(${gmail.url}/img/fresh-bg.png);width:8px;height:8px"></div>` +
      `<p><img src="${pixelDataUrl}" alt="data pixel"></p>`,
  });
  await app.settle();
  await app.row("Fresh images").click();
  await expect(app.conversationTitle).toHaveText("Fresh images");
  await expect.poll(() => loaded(app, "data pixel")).toBe(true);
  await app.settle();
  expect(await loaded(app, "remote pixel")).toBe(false);
  const hits = await imageHits(app);
  expect(hits).not.toContain("/img/fresh.png");
  expect(hits).not.toContain("/img/fresh-bg.png");

  // Back on, with it open: fetched at once.
  await app.press("Meta+,");
  await remoteImages(app).click();
  await expect(remoteImages(app)).toHaveAttribute("aria-checked", "true");
  await app.press("Escape");
  await expect
    .poll(() => imageHits(app))
    .toEqual(expect.arrayContaining(["/img/fresh.png", "/img/fresh-bg.png"]));
  await expect.poll(() => loaded(app, "remote pixel")).toBe(true);
});
