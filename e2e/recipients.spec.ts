// Recipient chips, autocomplete and its ranking, invalid addresses, and Cc/Bcc
// (SPEC Recipients and autocomplete, Keyboard).
import ADDRESSES from "../src-tauri/crates/sm-mime/tests/fixtures/email-addresses.json" with { type: "json" };
import type { App } from "./app.ts";
import type { Control, GmailMessage } from "./control.ts";
import { expect, test } from "./fixtures.ts";
import * as seeds from "./seeds.ts";
import { A, PERSONS } from "./seeds.ts";

// ------------------------------------------------------------------ helpers

/** Every address in a header, lower-cased, in order. */
function emails(header: string | undefined): string[] {
  return [...(header ?? "").matchAll(/[\w.+-]+@[\w-]+(?:\.[\w-]+)+/g)].map((m) =>
    m[0].toLowerCase(),
  );
}

/** The one message the app sent with this subject. */
async function sentWithSubject(
  gmail: Control,
  account: string,
  subject: string,
): Promise<GmailMessage> {
  const { messages } = await gmail.gmail(account);
  const found = messages.filter(
    (m) =>
      m.labels.includes("SENT") &&
      m.headers["x-supermonkey-draft"] !== undefined &&
      m.headers.subject === subject,
  );
  expect(found, `one sent message "${subject}"`).toHaveLength(1);
  const [m] = found;
  if (!m) throw new Error(`no sent message ${subject}`);
  return m;
}

/** Addresses of the chips in a field. */
async function chipAddresses(app: App, label: "To" | "Cc" | "Bcc"): Promise<string[]> {
  return app.chips(label).evaluateAll((els) => els.map((el) => el.getAttribute("data-chip") ?? ""));
}

/** The address shown in each open suggestion, in order. */
async function suggested(app: App): Promise<string[]> {
  return app
    .suggestions("To")
    .evaluateAll((els) => els.map((el) => el.querySelectorAll("span")[1]?.textContent ?? ""));
}

/** Fills Subject and body of the composer on screen. */
async function subjectAndBody(app: App, subject: string, body: string): Promise<void> {
  await app.press("Meta+Shift+s");
  await expect(app.subject).toBeFocused();
  await app.type(subject);
  await app.press("Meta+Shift+m");
  await expect(app.editor).toBeFocused();
  await app.type(body);
}

/** Sends the composer on screen once its autosave has run, and lets the outbox send it. */
async function sendSaved(app: App): Promise<void> {
  await app.settle();
  await app.press("Meta+Enter");
  await expect(app.editor).toHaveCount(0);
  await app.advance(10_000);
  await app.settle();
}

// ------------------------------------------------------------------ cases

test("RCP-01 autocomplete: at most six suggestions, keyboard choice, Esc stays in the field", async ({
  app,
  gmail,
}) => {
  await seeds.contacts(gmail, app);
  await app.compose();
  const to = app.field("To");

  // Eight contacts match "mar"; six show, each with name, address, and the match in bold.
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

  // ↓ ↓ ↑ highlights the second; Enter makes it a chip.
  const second = (await suggested(app))[1];
  await app.press("ArrowDown", "ArrowDown", "ArrowUp");
  await expect(app.suggestions().nth(1)).toHaveAttribute("aria-selected", "true");
  await app.press("Enter");
  expect(await chipAddresses(app, "To")).toEqual([second]);
  await expect(app.suggestions()).toHaveCount(0);
  await expect(to).toBeFocused();

  // Tab accepts the highlighted suggestion.
  await app.type("mar");
  await expect(app.suggestions().first()).toHaveAttribute("aria-selected", "true");
  const first = (await suggested(app))[0];
  await app.press("Tab");
  expect(await chipAddresses(app, "To")).toEqual([second, first]);
  await expect(to).toBeFocused();

  // Esc closes the list and keeps focus in To.
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
  // The contacts backfill reads sent mail's headers only (SPEC: format=metadata).
  const log = await gmail.requests(undefined, { account: A });
  expect(log.some((r) => r.params.format === "metadata")).toBe(true);

  await app.compose();
  const to = app.field("To");
  await app.type("s");
  await app.settle();
  // Sent tier (Sam, then Sara, whose only sent mail is 200 days old, so only the
  // backfill finds her), then received only, then co-recipient only. List-only
  // (Sly), no-reply, spam, and draft-only addresses never show.
  expect(await suggested(app)).toEqual([
    "sam@x.test",
    "sara@x.test",
    "sue@recv.test",
    "sid@cc.test",
  ]);

  // The account's own alias, a co-recipient on "To the alias", never shows.
  await to.fill("");
  await app.type("ames");
  await app.settle();
  expect(await suggested(app)).toEqual([]);

  // Never sent to and seen only on list mail: the list's sender (List Bot
  // <bot@list.test>) as well as Sly.
  await to.fill("");
  await app.type("list");
  await app.settle();
  expect(await suggested(app)).toEqual([]);

  // Matched at the start of the domain; never from the middle of the local part.
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
  // Addresses with no contacts, so no suggestion list opens.
  const chipAfter = async (typed: string, then: () => Promise<void>) => {
    await app.type(typed);
    await app.settle();
    await expect(app.suggestions()).toHaveCount(0);
    await then();
    await expect(app.chips("To").filter({ hasText: typed })).toHaveCount(1);
    // WebKit can leave the typed space behind in the emptied field.
    await expect(to).toHaveValue(/^\s*$/);
  };

  await chipAfter("kai@kite.test", () => app.type(","));
  await chipAfter("kay@kite.test", () => app.type(" "));

  // A space after text that is not a complete address leaves it as text.
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
  expect(await chipAddresses(app, "To")).toEqual([
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
  expect(await chipAddresses(app, "To")).toEqual(["one@x.test", "two@x.test"]);

  // The first Backspace selects the last chip; the second removes it.
  await app.press("Backspace");
  await expect(app.chips("To").last()).toHaveAttribute("data-selected", "true");
  expect(await chipAddresses(app, "To")).toEqual(["one@x.test", "two@x.test"]);
  await app.press("Backspace");
  expect(await chipAddresses(app, "To")).toEqual(["one@x.test"]);
  await app.press("Backspace", "Backspace");
  await expect(app.chips("To")).toHaveCount(0);

  await app.paste("a@x.test, Bob <b@y.test>; c@z.test\nd@w.test");
  expect(await chipAddresses(app, "To")).toEqual(["a@x.test", "b@y.test", "c@z.test", "d@w.test"]);
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

  // With the list open, Tab accepts the suggestion instead of moving. (First,
  // while To has never lost focus: its blur handler closes the list 120 ms later.)
  await app.type("mar");
  await expect(app.suggestions()).toHaveCount(6);
  const first = (await suggested(app))[0];
  await app.press("Tab");
  expect(await chipAddresses(app, "To")).toEqual([first]);
  await expect(to).toBeFocused();

  // To → Subject → body, and back.
  await app.press("Tab");
  await expect(app.subject).toBeFocused();
  await app.press("Tab");
  await expect(app.editor).toBeFocused();
  await app.press("Shift+Tab");
  await expect(app.subject).toBeFocused();
  await app.press("Shift+Tab");
  await expect(to).toBeFocused();

  // Through Cc when it is shown.
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

  // A new message has no Cc or Bcc row; Cmd+Shift+C shows Cc.
  await app.compose();
  await expect(app.field("Cc")).toHaveCount(0);
  await expect(app.field("Bcc")).toHaveCount(0);
  await app.press("Meta+Shift+c");
  await expect(app.field("Cc")).toBeVisible();
  await expect(app.field("Bcc")).toHaveCount(0);
  await expect(app.field("Cc")).toBeFocused();
  await app.press("Escape", "Escape");
  await expect(app.editor).toHaveCount(0);

  // Reply all on addr shows Cc, already filled.
  await app.row("Offsite venue").click();
  await expect(app.conversationTitle).toHaveText("Offsite venue");
  await app.settle();
  await app.reply("Enter");
  await expect(app.field("Cc")).toBeVisible();
  expect(await chipAddresses(app, "Cc")).toEqual(["dave@corp.test", "erin@corp.test"]);
  await app.press("Escape", "Escape", "Escape");
  await expect(app.conversationTitle).toHaveCount(0);

  // A Bcc recipient gets the message without showing in To or Cc.
  await app.compose();
  await app.type("kim@kite.test,");
  await app.press("Meta+Shift+b");
  await expect(app.field("Bcc")).toBeFocused();
  await app.type("bea@hidden.test,");
  expect(await chipAddresses(app, "Bcc")).toEqual(["bea@hidden.test"]);
  await subjectAndBody(app, "Blind copy", "Hello");
  await sendSaved(app);
  const m = await sentWithSubject(gmail, A, "Blind copy");
  expect(emails(m.headers.to)).toEqual(["kim@kite.test"]);
  expect(emails(m.headers.cc)).toEqual([]);
  expect(emails(m.headers.bcc)).toEqual(["bea@hidden.test"]);
});

test("RCP-08 new mail updates the suggestions", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  // Quin Quill gets two more messages, so as received-only contacts Quin (three)
  // outranks Quinn (one) until Ana sends to Quinn.
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

  // After a send to Quinn, Quinn is in the sent tier, above received-only Quin.
  const quinn = app.suggestions().filter({ hasText: "quinn@q.test" });
  await quinn.dispatchEvent("mousedown");
  expect(await chipAddresses(app, "To")).toEqual(["quinn@q.test"]);
  await subjectAndBody(app, "For Quinn", "Hello");
  await sendSaved(app);
  await sentWithSubject(gmail, A, "For Quinn");

  await app.compose();
  await app.type("qui");
  await app.settle();
  expect(await suggested(app)).toEqual(["quinn@q.test", "quin@people.test"]);
});

test("RCP-09 suggestions that answer after a chip is made stay closed", async ({ app, gmail }) => {
  await seeds.contacts(gmail, app);
  await app.compose();
  // Page timers wait for the test, so the autosave cannot close the list first.
  await app.page.clock.pauseAt(await app.now());
  const release = await app.park("autocomplete");
  await app.type("sam,");
  await expect(app.chips("To")).toHaveCount(1);
  await release();
  await expect.poll(() => app.page.evaluate<number>("window.__sm.pending().invokes")).toBe(0);
  await expect(app.suggestions()).toHaveCount(0);

  // Tab leaves the field without adding anyone.
  await app.press("Tab");
  await app.page.clock.resume();
  await app.page.clock.setFixedTime(await app.now());
  await app.settle();
  expect(await chipAddresses(app, "To")).toEqual(["sam"]);
});

test("RCP-10 the autosave leaves an address being typed alone", async ({ app, gmail }) => {
  await seeds.standard(gmail, app);
  await app.compose();
  await app.type("kim@kite.test,");
  await expect(app.chips("To")).toHaveCount(1);
  await app.type("lee@ki");
  // The autosave the chip scheduled fires while the next address is half typed.
  await app.page.clock.runFor(500);
  await app.settle();
  expect(await chipAddresses(app, "To")).toEqual(["kim@kite.test"]);
  await expect(app.field("To")).toHaveValue("lee@ki");
  await expect(app.field("To")).toBeFocused();
});

test("RCP-12 a chip removed while an address is being typed stays removed", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  await app.compose();
  // Addresses with no contacts, so no suggestion list opens.
  await app.type("kai@kite.test,kay@kite.test,lex@kite.test");
  await expect(app.chips("To")).toHaveCount(2);
  await app.settle();

  // The click on × takes focus from the field, which turns the typed text into a chip.
  await app.page.locator('[data-chip="kai@kite.test"]').hover();
  await app.page.getByRole("button", { name: "Remove kai@kite.test" }).click();
  await app.page.clock.runFor(200);
  await app.settle();
  expect(await chipAddresses(app, "To")).toEqual(["kay@kite.test", "lex@kite.test"]);
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
  // Each refused address shows in `danger`.
  const invalid = ADDRESSES.filter((c) => !c.valid);
  expect(
    await app
      .invalidChips("To")
      .evaluateAll((els) => els.map((el) => el.classList.contains("text-danger"))),
  ).toEqual(invalid.map(() => true));
  await subjectAndBody(app, "Table", "Hello");

  // Refused before anything closes, with a toast naming every refused address.
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

  // Fixed: with the chips cleared (Backspace selects a chip, a second removes
  // it) and a valid address typed, the message sends.
  await app.press("Meta+Shift+o");
  await expect(app.field("To")).toBeFocused();
  await app.press(...Array.from({ length: 2 * ADDRESSES.length }, () => "Backspace"));
  await expect(app.chips("To")).toHaveCount(0);
  await app.type("kim@kite.test,");
  await expect(app.invalidChips("To")).toHaveCount(0);
  await sendSaved(app);
  const m = await sentWithSubject(gmail, A, "Table");
  expect(emails(m.headers.to)).toEqual(["kim@kite.test"]);
});
