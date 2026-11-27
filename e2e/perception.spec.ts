import { AxeBuilder } from "@axe-core/playwright";

import type { App } from "./app.ts";
import { expect, test } from "./fixtures.ts";
import * as seeds from "./seeds.ts";

async function scanAccessible(app: App, screen: string): Promise<string[]> {
  await app.page.evaluate(async () => {
    const running = document
      .getAnimations()
      .filter((a) => a.effect?.getTiming().iterations !== Number.POSITIVE_INFINITY);
    await Promise.allSettled(running.map((a) => a.finished));
  });
  const results = await new AxeBuilder({ page: app.page })
    .exclude("iframe[data-frame]")
    .withTags(["wcag2a", "wcag2aa"])
    .analyze();
  return results.violations.map(
    (v) =>
      `[${screen}] ${v.id} (${v.impact ?? "unknown"}): ${v.nodes.map((n) => n.html).join(" | ")}`,
  );
}

async function expectNoHorizontalOverflow(app: App, screen: string): Promise<void> {
  const overflow = await app.page.evaluate(() => ({
    scrollWidth: document.documentElement.scrollWidth,
    clientWidth: document.documentElement.clientWidth,
  }));
  expect(
    overflow.scrollWidth,
    `${screen} must not overflow horizontally (${overflow.scrollWidth} > ${overflow.clientWidth})`,
  ).toBeLessThanOrEqual(overflow.clientWidth + 1);
}

test("PER-01 primary screens pass axe WCAG A/AA accessibility checks", async ({ app, gmail }) => {
  const violations: string[] = [];

  await app.open();
  await app.settle();
  await expect(app.page.getByRole("button", { name: "Add a Gmail account" })).toBeVisible();
  violations.push(...(await scanAccessible(app, "sign-in")));

  await app.restart();
  await seeds.standard(gmail, app);
  await expect(app.row("Person 1")).toHaveCount(1);
  violations.push(...(await scanAccessible(app, "inbox")));

  await app.row("Q4 plan").click();
  await expect(app.conversationTitle).toHaveText("Q4 plan");
  await app.settle();
  violations.push(...(await scanAccessible(app, "conversation")));
  await app.press("Escape");

  await app.compose();
  await expect(app.page.getByRole("heading", { name: "New Message" })).toBeVisible();
  violations.push(...(await scanAccessible(app, "composer")));
  await app.press("Escape", "Escape");

  await app.press("Meta+k");
  await expect(app.palette).toBeVisible();
  violations.push(...(await scanAccessible(app, "command palette")));
  await app.press("Escape");

  await app.press("Meta+,");
  await expect(app.sheet("Settings")).toBeVisible();
  violations.push(...(await scanAccessible(app, "settings")));
  await app.press("Escape");

  await app.press("?");
  await expect(app.sheet("Shortcuts")).toBeVisible();
  violations.push(...(await scanAccessible(app, "shortcuts")));
  await app.press("Escape");

  expect(violations, "axe WCAG A/AA violations across primary screens").toEqual([]);
});

test("PER-02 primary screens hold up without horizontal overflow in a small window", async ({
  app,
  gmail,
}) => {
  await app.page.setViewportSize({ width: 760, height: 540 });
  await seeds.standard(gmail, app);
  await gmail.deliver({
    account: seeds.A,
    from: "Very Long Sender Name With Many Words <long.sender.address@subdomain.example.test>",
    subject:
      "SuperLongUnbrokenSubjectTokenWithoutAnySpacesToVerifyEllipsisAndNoHorizontalOverflowInSmallWindow",
    text: "Long line body text.",
    labels: ["INBOX", "UNREAD", "CATEGORY_PERSONAL"],
  });
  await app.settle();
  await expect(app.row("SuperLongUnbroken")).toHaveCount(1);
  await expectNoHorizontalOverflow(app, "inbox (small window)");

  await app.row("Q4 plan").click();
  await expect(app.conversationTitle).toHaveText("Q4 plan");
  await app.settle();
  await expectNoHorizontalOverflow(app, "conversation (small window)");
  await app.press("Escape");

  await app.compose();
  await expect(app.page.getByRole("heading", { name: "New Message" })).toBeVisible();
  await expectNoHorizontalOverflow(app, "composer (small window)");
  await app.press("Escape", "Escape");

  await app.press("Meta+,");
  await expect(app.sheet("Settings")).toBeVisible();
  await expectNoHorizontalOverflow(app, "settings (small window)");
  await app.press("Escape");
});
