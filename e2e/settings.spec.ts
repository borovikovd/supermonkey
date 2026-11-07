// The Settings sheet and the remote images setting.
import type { Locator } from "@playwright/test";

import type { App } from "./app.ts";
import { expect, test } from "./fixtures.ts";
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

  // Back on, with a message open: its remote images load at once.
  await app.row("Links").click();
  await expect(app.conversationTitle).toHaveText("Links");
  await expect.poll(() => frameCsp(app)).toContain("img-src data:");
  await app.press("Meta+,");
  await remoteImages(app).click();
  await expect(remoteImages(app)).toHaveAttribute("aria-checked", "true");
  await app.press("Escape");
  await expect.poll(() => frameCsp(app)).not.toContain("img-src");
  await expect.poll(() => loaded(app, "remote pixel")).toBe(true);
});

const channels = (css: string): number[] =>
  (css.match(/\d+(\.\d+)?/g) ?? ["0", "0", "0"]).slice(0, 3).map(Number);

/** Relative luminance of a CSS rgb() color (WCAG). */
function luminance(css: string): number {
  const [r = 0, g = 0, b = 0] = channels(css).map((v) => {
    const c = v / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** WCAG contrast ratio between two CSS colors. */
function contrast(a: string, b: string): number {
  const [x, y] = [luminance(a), luminance(b)];
  return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05);
}

// A color and a text size were both named `hint`; the color won, so hints were 13px and chip keys near-white.
test("SET-03 hint text is 11px, the shortcut chips are readable, and the sheet's rows never overlap", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  const hintBar = app.page.getByText("E to Mark Done").locator("..");
  await expect(hintBar).toHaveCSS("font-size", "11px");
  await expect(
    app.page.getByRole("button", { name: "Important" }).locator("xpath=following-sibling::*[1]"),
  ).toHaveCSS("font-size", "11px");

  await app.press("?");
  const sheet = app.sheet("Keyboard shortcuts");
  await expect(sheet).toBeVisible();
  const chips = sheet.locator("kbd");
  expect(await chips.count()).toBeGreaterThan(20);
  const looks = await chips.evaluateAll((els) =>
    els.map((el) => {
      const css = getComputedStyle(el);
      return {
        text: el.textContent ?? "",
        size: css.fontSize,
        fg: css.color,
        bg: css.backgroundColor,
      };
    }),
  );
  for (const l of looks) {
    expect(l.size, l.text).toBe("11px");
    expect(contrast(l.fg, l.bg), `${l.text} contrast`).toBeGreaterThanOrEqual(3);
  }

  // The sheet's rows never overlap, even when a description wraps.
  const overlaps = await sheet.locator("kbd").evaluateAll((els) => {
    const rows = els.map((k) => k.parentElement).filter((r): r is HTMLElement => r !== null);
    const out: string[] = [];
    for (const [i, row] of rows.entries()) {
      const next = rows[i + 1];
      if (!next || next.parentElement !== row.parentElement) continue;
      const a = row.getBoundingClientRect();
      const b = next.getBoundingClientRect();
      const text = row.querySelector("span");
      const t = text?.getBoundingClientRect();
      if (a.bottom > b.top + 0.5 || (t && t.bottom > a.bottom + 0.5)) out.push(row.innerText);
    }
    return out;
  });
  expect(overlaps).toEqual([]);
});

// The message frame blocked only images, so a web font still told the sender, and the quote frame blocked nothing.
test("SET-05 with remote images off, nothing remote loads in a message or a reply's quote", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  await app.press("Meta+,");
  await remoteImages(app).click();
  await expect(remoteImages(app)).toHaveAttribute("aria-checked", "false");
  await app.press("Escape");

  await gmail.deliver({
    account: A,
    from: "Tia Tracker <tia@tracker.test>",
    subject: "Tracked",
    html:
      `<style>@import url(${gmail.url}/img/import.css);` +
      `@font-face{font-family:tracked;src:url(${gmail.url}/img/font.woff)}</style>` +
      `<p style="font-family:tracked">Opened yet?</p>` +
      `<p><img src="${gmail.url}/img/tracked.png" alt="remote pixel"></p>` +
      `<div style="background-image:url(${gmail.url}/img/tracked-bg.png);width:8px;height:8px"></div>`,
  });
  await app.row("Tracked").click();
  await expect(
    app.conversation.locator("iframe[data-frame]").contentFrame().locator("body"),
  ).toContainText("Opened yet?");
  await app.settle();
  expect(await imageHits(app)).toEqual([]);

  await app.press("r");
  await app.page.getByTitle("Show quoted text").click();
  await expect(
    app.page.locator('iframe[title="quoted"]').contentFrame().locator("body"),
  ).toContainText("Opened yet?");
  await app.settle();
  expect(await imageHits(app)).toEqual([]);
});

// Esc in a field of the Split Inbox sheet did nothing, and with a composer
// open, Esc went to the composer while Settings stayed open over it.
test("SET-06 Esc closes a sheet from one of its fields, and a sheet open over a composer", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);

  // A field in the Split Inbox sheet has focus: Esc closes the sheet.
  await app.press("Meta+k");
  await app.type("Split Inbox settings");
  await app.press("Enter");
  const splits = app.sheet(`Split Inbox · ${A}`);
  await expect(splits).toBeVisible();
  await splits.getByRole("textbox").first().click();
  await expect(splits.getByRole("textbox").first()).toBeFocused();
  await app.press("Escape");
  await expect(splits).toHaveCount(0);

  // Settings over a new message: Esc closes Settings, and the composer stays.
  const newMessage = app.page.getByRole("heading", { name: "New Message" });
  await app.compose();
  await app.press("Meta+,");
  await expect(app.sheet("Settings")).toBeVisible();
  await app.press("Escape");
  await expect(app.sheet("Settings")).toHaveCount(0);
  await expect(newMessage).toBeVisible();
  // Then the composer's own Esc, twice, closes it.
  await app.press("Escape", "Escape");
  await expect(newMessage).toHaveCount(0);
});
