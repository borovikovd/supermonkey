import ADDRESSES from "../src-tauri/tests/fixtures/email-addresses.json" with { type: "json" };
import type { App } from "./app.ts";
import { expect, test } from "./fixtures.ts";
import { emails, sentWithSubject } from "./helpers.ts";
import * as seeds from "./seeds.ts";
import { A, PERSONS } from "./seeds.ts";

async function suggested(app: App): Promise<string[]> {
  return app
    .suggestions("To")
    .evaluateAll((els) => els.map((el) => el.querySelectorAll("span")[1]?.textContent ?? ""));
}

async function subjectAndBody(app: App, subject: string, body: string): Promise<void> {
  await app.press("Meta+Shift+s");
  await expect(app.subject).toBeFocused();
  await app.type(subject);
  await app.press("Meta+Shift+m");
  await expect(app.editor).toBeFocused();
  await app.type(body);
}

test("RCP-01 autocomplete: at most six suggestions, keyboard choice, Esc stays in the field", async ({
  app,
  gmail,
}) => {
  await seeds.contacts(gmail, app);
  await app.compose();
  const to = app.field("To");

  await app.type("mar");
  await expect(app.suggestions()).toHaveCount(6);
  const options = await app
    .suggestions()
    .evaluateAll((els) =>
      els.map((el) => [
        el.querySelectorAll("span")[0]?.textContent ?? "",
        el.querySelectorAll("span")[1]?.textContent ?? "",
        el.querySelector("b")?.textContent ?? "",
      ]),
    );
  for (const [name, address, bold] of options) {
    expect(name).toMatch(/^Mark M\w+$/);
    expect(address).toMatch(/^\w+@marks\.test$/);
    expect(bold).toMatch(/^mar$/i);
  }

  const second = (await suggested(app))[1];
  await app.press("ArrowDown", "ArrowDown", "ArrowUp");
  await expect(app.suggestions().nth(1)).toHaveAttribute("aria-selected", "true");
  await app.press("Enter");
  expect(await app.chipAddresses("To")).toEqual([second]);
  await expect(app.suggestions()).toHaveCount(0);
  await expect(to).toBeFocused();

  await app.type("mar");
  await expect(app.suggestions().first()).toHaveAttribute("aria-selected", "true");
  const first = (await suggested(app))[0];
  await app.press("Tab");
  expect(await app.chipAddresses("To")).toEqual([second, first]);
  await expect(to).toBeFocused();

  await app.type("mar");
  await expect(app.suggestions().first()).toBeVisible();
  await app.press("Escape");
  await expect(app.suggestions()).toHaveCount(0);
  await expect(to).toBeFocused();
  await app.type("k");
  await expect(to).toHaveValue("mark");
  await expect(app.page.getByRole("heading", { name: "New Message" })).toBeVisible();
});

test("RCP-02 ranking and exclusions come from synced mail", async ({ app, gmail }) => {
  await seeds.contacts(gmail, app);
  await app.settle();
  const log = await gmail.requests(undefined, { account: A });
  expect(log.some((r) => r.params.format === "metadata")).toBe(true);

  await app.compose();
  const to = app.field("To");
  await app.type("s");
  await app.settle();
  expect(await suggested(app)).toEqual([
    "sam@x.test",
    "sara@x.test",
    "sue@recv.test",
    "sid@cc.test",
  ]);

  await to.fill("");
  await app.type("ames");
  await app.settle();
  expect(await suggested(app)).toEqual([]);

  await to.fill("");
  await app.type("list");
  await app.settle();
  expect(await suggested(app)).toEqual([]);

  await to.fill("");
  await app.type("recv");
  await app.settle();
  expect(await suggested(app)).toContain("sue@recv.test");
  await to.fill("");
  await app.type("ue");
  await app.settle();
  expect(await suggested(app)).not.toContain("sue@recv.test");
});

test("RCP-03 ways to turn typed text into a chip", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  await app.compose();
  const to = app.field("To");
  const chipAfter = async (typed: string, then: () => Promise<void>) => {
    await app.type(typed);
    await app.settle();
    await expect(app.suggestions()).toHaveCount(0);
    await then();
    await expect(app.chips("To").filter({ hasText: typed })).toHaveCount(1);
    await expect(to).toHaveValue(/^\s*$/);
  };

  await chipAfter("kai@kite.test", () => app.type(","));
  await chipAfter("kay@kite.test", () => app.type(" "));

  await app.type("kai ");
  await app.settle();
  await expect(app.chips("To")).toHaveCount(2);
  await expect(to).toHaveValue(/^\s*kai\s*$/);
  await to.fill("");

  await chipAfter("lex@kite.test", () => app.press("Tab"));
  await to.focus();
  await chipAfter("lou@kite.test", () => app.press("Enter"));
  await chipAfter("zed@z.test", () =>
    app.page.getByRole("heading", { name: "New Message" }).click(),
  );
  expect(await app.chipAddresses("To")).toEqual([
    "kai@kite.test",
    "kay@kite.test",
    "lex@kite.test",
    "lou@kite.test",
    "zed@z.test",
  ]);
});

test("RCP-04 Backspace and paste", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  await app.compose();
  await app.type("one@x.test,two@x.test,");
  expect(await app.chipAddresses("To")).toEqual(["one@x.test", "two@x.test"]);

  await app.press("Backspace");
  await expect(app.chips("To").last()).toHaveAttribute("data-selected", "true");
  expect(await app.chipAddresses("To")).toEqual(["one@x.test", "two@x.test"]);
  await app.press("Backspace");
  expect(await app.chipAddresses("To")).toEqual(["one@x.test"]);
  await app.press("Backspace", "Backspace");
  await expect(app.chips("To")).toHaveCount(0);

  await app.paste("a@x.test, Bob <b@y.test>; c@z.test\nd@w.test");
  expect(await app.chipAddresses("To")).toEqual(["a@x.test", "b@y.test", "c@z.test", "d@w.test"]);
  await expect(app.chips("To").nth(1)).toHaveText(/^Bob/);
});

test("RCP-06 Tab moves between fields unless the suggestion list is open", async ({
  app,
  gmail,
}) => {
  await seeds.contacts(gmail, app);
  await app.compose();
  const to = app.field("To");
  const cc = app.field("Cc");

  await app.type("mar");
  await expect(app.suggestions()).toHaveCount(6);
  const first = (await suggested(app))[0];
  await app.press("Tab");
  expect(await app.chipAddresses("To")).toEqual([first]);
  await expect(to).toBeFocused();

  await app.press("Tab");
  await expect(app.subject).toBeFocused();
  await app.press("Tab");
  await expect(app.editor).toBeFocused();
  await app.press("Shift+Tab");
  await expect(app.subject).toBeFocused();
  await app.press("Shift+Tab");
  await expect(to).toBeFocused();

  await app.press("Meta+Shift+c");
  await expect(cc).toBeFocused();
  await app.press("Meta+Shift+o");
  await expect(to).toBeFocused();
  await app.press("Tab");
  await expect(cc).toBeFocused();
  await app.press("Tab");
  await expect(app.subject).toBeFocused();
  await app.press("Tab");
  await expect(app.editor).toBeFocused();
  await app.press("Shift+Tab");
  await expect(app.subject).toBeFocused();
  await app.press("Shift+Tab");
  await expect(cc).toBeFocused();
  await app.press("Shift+Tab");
  await expect(to).toBeFocused();
});

test("RCP-07 Cc and Bcc stay hidden until used", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);

  await app.compose();
  await expect(app.field("Cc")).toHaveCount(0);
  await expect(app.field("Bcc")).toHaveCount(0);
  await app.press("Meta+Shift+c");
  await expect(app.field("Cc")).toBeVisible();
  await expect(app.field("Bcc")).toHaveCount(0);
  await expect(app.field("Cc")).toBeFocused();
  await app.press("Escape", "Escape");
  await expect(app.editor).toHaveCount(0);

  await app.row("Offsite venue").click();
  await expect(app.conversationTitle).toHaveText("Offsite venue");
  await app.settle();
  await app.reply("Enter");
  await expect(app.field("Cc")).toBeVisible();
  expect(await app.chipAddresses("Cc")).toEqual(["dave@corp.test", "erin@corp.test"]);
  await app.press("Escape", "Escape", "Escape");
  await expect(app.conversationTitle).toHaveCount(0);

  await app.compose();
  await app.type("kim@kite.test,");
  await app.press("Meta+Shift+b");
  await expect(app.field("Bcc")).toBeFocused();
  await app.type("bea@hidden.test,");
  expect(await app.chipAddresses("Bcc")).toEqual(["bea@hidden.test"]);
  await subjectAndBody(app, "Blind copy", "Hello");
  await app.send();
  const m = await sentWithSubject(gmail, A, "Blind copy");
  expect(emails(m.headers.to)).toEqual(["kim@kite.test"]);
  expect(emails(m.headers.cc)).toEqual([]);
  expect(emails(m.headers.bcc)).toEqual(["bea@hidden.test"]);
});

test("RCP-08 new mail updates the suggestions", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  const fromQuin = (n: number) =>
    gmail.deliver({
      account: A,
      from: PERSONS[1] ?? "",
      to: [A],
      subject: `Quin again ${n}`,
      text: "More from Quin.",
      poll: false,
    });
  await fromQuin(1);
  await fromQuin(2);
  await gmail.deliver({
    account: A,
    from: "Quinn Q <quinn@q.test>",
    to: [A],
    subject: "Hello from Quinn",
    text: "Hi Ana.",
  });
  await app.settle();

  await app.compose();
  await app.type("qui");
  await app.settle();
  expect(await suggested(app)).toEqual(["quin@people.test", "quinn@q.test"]);

  const quinn = app.suggestions().filter({ hasText: "quinn@q.test" });
  await quinn.dispatchEvent("mousedown");
  expect(await app.chipAddresses("To")).toEqual(["quinn@q.test"]);
  await subjectAndBody(app, "For Quinn", "Hello");
  await app.send();
  await sentWithSubject(gmail, A, "For Quinn");

  await app.compose();
  await app.type("qui");
  await app.settle();
  expect(await suggested(app)).toEqual(["quinn@q.test", "quin@people.test"]);
});

test("RCP-09 suggestions that answer after a chip is made stay closed", async ({ app, gmail }) => {
  await seeds.contacts(gmail, app);
  await app.compose();
  await app.page.clock.pauseAt(await app.now());
  const release = await app.park("autocomplete");
  await app.type("sam,");
  await expect(app.chips("To")).toHaveCount(1);
  await release();
  await expect.poll(() => app.page.evaluate<number>("window.__sm.pending().invokes")).toBe(0);
  await expect(app.suggestions()).toHaveCount(0);

  await app.press("Tab");
  await app.page.clock.resume();
  await app.page.clock.setFixedTime(await app.now());
  await app.settle();
  expect(await app.chipAddresses("To")).toEqual(["sam"]);
});

test("RCP-10 the autosave leaves an address being typed alone", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  await app.compose();
  await app.type("kim@kite.test,");
  await expect(app.chips("To")).toHaveCount(1);
  await app.type("lee@ki");
  await app.page.clock.runFor(500);
  await app.settle();
  expect(await app.chipAddresses("To")).toEqual(["kim@kite.test"]);
  await expect(app.field("To")).toHaveValue("lee@ki");
  await expect(app.field("To")).toBeFocused();
});

test("RCP-12 a chip removed while an address is being typed stays removed", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  await app.compose();
  await app.type("kai@kite.test,kay@kite.test,lex@kite.test");
  await expect(app.chips("To")).toHaveCount(2);
  await app.settle();

  await app.page.locator('[data-chip="kai@kite.test"]').hover();
  await app.page.getByRole("button", { name: "Remove kai@kite.test" }).click();
  await app.page.clock.runFor(200);
  await app.settle();
  expect(await app.chipAddresses("To")).toEqual(["kay@kite.test", "lex@kite.test"]);
  await expect(app.field("To")).toHaveValue("");
});

test("RCP-11 every address the core would refuse shows in danger and blocks send until it is fixed", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  await app.compose();
  await app.paste(ADDRESSES.map((c) => c.address).join("\n"));
  await expect(app.chips("To")).toHaveCount(ADDRESSES.length);
  const flagged = await app
    .chips("To")
    .evaluateAll((els) =>
      els.map((el) => [el.getAttribute("data-chip") ?? "", el.getAttribute("aria-invalid")]),
    );
  expect(Object.fromEntries(flagged)).toEqual(
    Object.fromEntries(ADDRESSES.map((c) => [c.address, c.valid ? null : "true"])),
  );
  const invalid = ADDRESSES.filter((c) => !c.valid);
  expect(
    await app
      .invalidChips("To")
      .evaluateAll((els) => els.map((el) => el.classList.contains("text-danger"))),
  ).toEqual(invalid.map(() => true));
  await subjectAndBody(app, "Table", "Hello");

  await app.settle();
  await app.press("Meta+Enter");
  const refused = app.errorToasts.filter({ hasText: "Fix these addresses" });
  await expect(refused).toBeVisible();
  const named = await refused.innerText();
  expect(invalid.filter((c) => !named.includes(c.address))).toEqual([]);
  await expect(app.editor).toBeVisible();
  await app.advance(10_000);
  await app.settle();
  await expect(app.toast("Sending…")).toHaveCount(0);
  expect(await gmail.requests("drafts.send")).toEqual([]);

  await app.press("Meta+Shift+o");
  await expect(app.field("To")).toBeFocused();
  await app.press(...Array.from({ length: 2 * ADDRESSES.length }, () => "Backspace"));
  await expect(app.chips("To")).toHaveCount(0);
  await app.type("kim@kite.test,");
  await expect(app.invalidChips("To")).toHaveCount(0);
  await app.send();
  const m = await sentWithSubject(gmail, A, "Table");
  expect(emails(m.headers.to)).toEqual(["kim@kite.test"]);
});
