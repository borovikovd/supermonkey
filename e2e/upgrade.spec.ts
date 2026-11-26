import path from "node:path";

import { attachment, files } from "./files.ts";
import { expect, test } from "./fixtures.ts";
import { liveMessageHas } from "./helpers.ts";
import { attachments, htmlOf, parseMime } from "./mime.ts";
import * as seeds from "./seeds.ts";
import { A, B } from "./seeds.ts";

const DATA_0_7_2 = path.resolve(import.meta.dirname, "../src-tauri/tests/fixtures/data-0.7.2");

test("UPG-01 an update from 0.7.2 keeps the accounts, snoozes, splits, drafts and settings, and sends what was still queued", async ({
  app,
  gmail,
}) => {
  const s = await seeds.drafts(gmail, app, { connect: [], open: false });
  await gmail.deliver({
    account: A,
    from: "Rae Report <rae@report.test>",
    subject: "Report to forward",
    text: "The report is attached.",
    attachments: [attachment(files.reportPdf)],
    poll: false,
  });
  await gmail.earlierData(DATA_0_7_2);
  await app.open();

  await expect(app.account(A)).toHaveCount(1);
  await expect(app.account(B)).toHaveCount(1);
  await expect(app.signInMarker(A)).toHaveCount(0);
  await expect(app.row("Person 2")).toHaveCount(1);
  await expect(app.row("Person 1"), "archived before the update").toHaveCount(0);
  await app.advance(10_000);
  await app.settle();
  await app.expectNoErrorToasts();

  const a = await gmail.gmail(A);
  const thread = (key: string) => s.thread(key).threadId;
  expect(liveMessageHas(a, thread("p1"), "INBOX"), "the archive lands").toBe(false);
  expect(liveMessageHas(a, thread("p4"), "STARRED"), "the star lands").toBe(true);
  expect(liveMessageHas(a, thread("p5"), "INBOX"), "a snooze lands").toBe(false);
  expect(liveMessageHas(a, thread("p6"), "INBOX"), "the other snooze lands").toBe(false);
  const label = a.labels.find((l) => l.name === "Upgrade kept");
  expect(label, "the label made before the update exists").toBeDefined();
  expect(liveMessageHas(a, thread("p2"), label?.id ?? "missing")).toBe(true);
  const sent = a.messages.filter(
    (m) => m.labels.includes("SENT") && m.headers.subject === "Sent before the update",
  );
  expect(sent, "the mail waiting in the outbox goes out once").toHaveLength(1);
  const drafts = a.drafts.map((d) => parseMime(d.message.raw));
  expect(drafts.map((d) => htmlOf(d)).join("\n")).toContain("Draft written before the update");
  const filesOn = (text: string) =>
    attachments(drafts.find((d) => htmlOf(d).includes(text)) ?? parseMime("")).map(
      (p) => p.filename,
    );
  expect(filesOn("Forward written before the update")).toEqual(["report.pdf"]);
  expect(filesOn("My text and more"), "a draft made in Gmail keeps its file").toEqual(["spec.pdf"]);
  expect(a.drafts, "seven drafts from Gmail and two written here").toHaveLength(9);

  const sections = await app.sections();
  expect(sections.map((x) => x.name)).toEqual(["VIP", "Important", "Other"]);
  expect(sections[0]?.subjects.toSorted()).toEqual(["Offsite venue", "Q4 plan", "Your receipt"]);

  await app.press("g", "h");
  await expect(app.row("Person 5")).toHaveCount(1);
  await expect(app.row("Person 6")).toHaveCount(1);
  await expect(app.rows).toHaveCount(2);
  await app.press("g", "d");
  await expect(app.row("Draft written before the update")).toHaveCount(1);
  await expect(app.row("Forward written before the update")).toHaveCount(1);

  await app.press("Meta+,");
  const settings = app.sheet("Settings");
  await expect(settings.getByRole("switch", { name: "Load remote images" })).toHaveAttribute(
    "aria-checked",
    "false",
  );
  await expect(
    app.settingsBlock(B).getByRole("switch", { name: "Mute notifications" }),
  ).toHaveAttribute("aria-checked", "true");
  await expect(
    app.settingsBlock(A).getByRole("switch", { name: "Mute notifications" }),
  ).toHaveAttribute("aria-checked", "false");
});
