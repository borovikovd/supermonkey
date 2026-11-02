// Checks the harness itself: seeding, the engine start, Gmail-side asserts,
// and add account through the fake OAuth.
import { expect, test } from "./fixtures.ts";

const ME = "me@example.com";

test("seeded mail shows and an archive reaches Gmail", async ({ app, gmail }) => {
  await gmail.account(ME, "Me");
  await gmail.deliver({ account: ME, from: "Ada Lovelace <ada@example.com>", subject: "Hello" });
  await app.open();
  await expect(app.row("Hello")).toBeVisible();
  await app.press("j", "e");
  await expect(app.row("Hello")).toHaveCount(0);
  await app.eventually(ME, (s) => s.messages.every((m) => !m.labels.includes("INBOX")));
});

test("add account signs in through the browser", async ({ app, gmail }) => {
  await gmail.account(ME, "Me");
  await gmail.user("sam@example.com", "Sam");
  await gmail.nextSignIn({ email: "sam@example.com" });
  await app.open();
  await app.press("Meta+k");
  await app.type("Add account");
  await app.press("Enter");
  await expect(app.account("sam@example.com")).toBeVisible();
});
