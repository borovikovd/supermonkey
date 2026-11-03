// Attachments (COVERAGE 5.14, SPEC Reading, Drafts, Addressing, Send and undo):
// attach and send, download on demand, offline forwards, the MIME tree.
import { readFileSync } from "node:fs";

import type { App } from "./app.ts";
import type { Control, GmailMessage } from "./control.ts";
import { files } from "./files.ts";
import { expect, test } from "./fixtures.ts";
import { attachments, findPart, htmlOf, parseMime, shape, type MimePart } from "./mime.ts";
import * as seeds from "./seeds.ts";
import { A } from "./seeds.ts";

/** A's sent messages, parsed, by decoded subject. */
async function sent(gmail: Control): Promise<{ message: GmailMessage; mime: MimePart }[]> {
  return (await gmail.gmail(A)).messages
    .filter((m) => m.labels.includes("SENT"))
    .map((message) => ({ message, mime: parseMime(message.raw) }));
}

async function sentWithSubject(gmail: Control, subject: string): Promise<MimePart> {
  const found = (await sent(gmail)).filter((s) => s.mime.headers.subject === subject);
  expect(found, `one sent message "${subject}"`).toHaveLength(1);
  const [first] = found;
  if (!first) throw new Error(`no sent message "${subject}"`);
  return first.mime;
}

/** Files by name, with their decoded bytes and content type. */
function filesOf(mime: MimePart): Record<string, { type: string; bytes: Buffer }> {
  return Object.fromEntries(
    attachments(mime).map((p) => [p.filename ?? "", { type: p.contentType, bytes: p.body }]),
  );
}

/** Starts a new message to Kim with this subject and body; the composer is ready to send. */
async function newMessage(app: App, subject: string, body: string): Promise<void> {
  await app.compose();
  await app.type("kim@kite.test,");
  await expect(app.chips("To")).toHaveCount(1);
  await app.subject.fill(subject);
  await app.press("Meta+Shift+m");
  await expect(app.editor).toBeFocused();
  await app.type(body);
}

test.describe("attachments", () => {
  test("ATT-01 attach files and send them", async ({ app, gmail }) => {
    await seeds.standard(gmail, app);
    await newMessage(app, "Files attached", "Here are the files.");

    await app.chooseFiles(() => app.press("Meta+Shift+a"), [files.reportPdf, files.photoPng]);
    await expect(app.page.getByRole("button", { name: "Remove report.pdf" })).toBeVisible();
    await expect(app.page.getByRole("button", { name: "Remove photo.png" })).toBeVisible();
    await app.chooseFiles(() => app.page.getByTitle("Attach (⌘⇧A)").click(), [files.notesTxt]);
    await expect(app.page.getByRole("button", { name: "Remove notes.txt" })).toBeVisible();
    await app.page.getByRole("button", { name: "Remove photo.png" }).click();
    await expect(app.page.getByRole("button", { name: "Remove photo.png" })).toHaveCount(0);
    await expect(app.page.getByText("report.pdf")).toBeVisible();
    await expect(app.page.getByText("notes.txt")).toBeVisible();

    await app.settle();
    const drafts = (await gmail.gmail(A)).drafts;
    expect(drafts).toHaveLength(1);
    const draft = filesOf(parseMime(drafts[0]?.message.raw ?? ""));
    expect(Object.keys(draft).toSorted()).toEqual(["notes.txt", "report.pdf"]);

    await app.press("Meta+Enter");
    await app.advance(10_000);
    await app.settle();
    const out = filesOf(await sentWithSubject(gmail, "Files attached"));
    expect(Object.keys(out).toSorted()).toEqual(["notes.txt", "report.pdf"]);
    expect(out["report.pdf"]?.type).toBe("application/pdf");
    expect(out["report.pdf"]?.bytes.equals(files.reportPdf.buffer)).toBe(true);
    expect(out["notes.txt"]?.bytes.equals(files.notesTxt.buffer)).toBe(true);
  });

  test("ATT-02 received attachments download only when opened", async ({ app, gmail }) => {
    await seeds.standard(gmail, app);
    // The connect sync may fetch inline cid: images with their bodies (SPEC Reading), nothing else.
    const synced = (await gmail.requests("attachments.get")).map((r) => r.path);

    await app.row("Contract").click();
    const chip = app.conversation.getByRole("button", { name: /contract\.pdf/ });
    await expect(chip).toContainText("contract.pdf");
    // The inline image is part of the body, not a file chip.
    await expect(app.conversation.getByRole("button", { name: /logo\.png/ })).toHaveCount(0);
    await app.settle();
    const opened = (await gmail.requests("attachments.get")).length;

    await chip.click();
    await app.settle();
    const first = await gmail.requests("attachments.get");
    expect(first).toHaveLength(opened + 1);
    const download = first.at(-1);
    expect(download?.answer).toBe(200);
    // That file was not fetched before it was opened.
    expect(synced).not.toContain(download?.path);
    const { openedFiles } = await gmail.platform();
    expect(openedFiles).toHaveLength(1);
    expect(readFileSync(openedFiles[0] ?? "").equals(files.contractPdf.buffer)).toBe(true);

    await chip.click();
    await app.settle();
    expect(await gmail.requests("attachments.get")).toHaveLength(opened + 1);
    const again = (await gmail.platform()).openedFiles;
    expect(again).toHaveLength(2);
    expect(readFileSync(again[1] ?? "").equals(files.contractPdf.buffer)).toBe(true);
  });

  test("ATT-03 a forward made offline still sends its attachments", async ({ app, gmail }) => {
    const s = await seeds.standard(gmail, app);
    const synced = new Set((await gmail.requests("attachments.get")).map((r) => r.path));

    await gmail.offline(true);
    await app.row("Contract").click();
    await expect(app.conversationTitle).toHaveText("Contract");
    await app.reply("f");
    await app.press("Meta+Shift+o");
    await app.type("kim@kite.test,");
    await expect(app.chips("To")).toHaveCount(1);
    await app.settle();
    await app.press("Meta+Enter");
    await app.advance(10_000);
    await expect(app.toast("Email delayed")).toBeVisible();

    await gmail.offline(false);
    await app.settle();
    const forward = (await sent(gmail)).filter(
      (m) => m.message.threadId === s.thread("fwd").threadId,
    );
    expect(forward).toHaveLength(1);
    const out = filesOf(forward[0]?.mime ?? parseMime(""));
    expect(out["contract.pdf"]?.bytes.equals(files.contractPdf.buffer)).toBe(true);

    // The file was downloaded before the draft was written and sent.
    const ok = (await gmail.requests()).filter(
      (r) => typeof r.answer === "number" && r.answer < 300,
    );
    const download = ok.find((r) => r.api === "attachments.get" && !synced.has(r.path));
    const upsert = ok.find((r) => r.api === "drafts.create" || r.api === "drafts.update");
    const send = ok.find((r) => r.api === "drafts.send");
    expect(download).toBeDefined();
    expect(upsert).toBeDefined();
    expect(send).toBeDefined();
    expect(download?.seq ?? Infinity).toBeLessThan(upsert?.seq ?? -1);
    expect(download?.seq ?? Infinity).toBeLessThan(send?.seq ?? -1);
  });

  test("ATT-04 MIME tree shape and encoded headers", async ({ app, gmail }) => {
    const s = await seeds.standard(gmail, app);
    const plainSubject = "Grüße – 日本";

    await newMessage(app, plainSubject, "No files here.");
    await app.settle();
    await app.press("Meta+Enter");
    await expect(app.page.getByRole("heading", { name: "New Message" })).toHaveCount(0);

    await newMessage(app, "One file", "One file here.");
    await app.chooseFiles(() => app.press("Meta+Shift+a"), [files.reportPdf]);
    await expect(app.page.getByRole("button", { name: "Remove report.pdf" })).toBeVisible();
    await app.settle();
    await app.press("Meta+Enter");
    await expect(app.page.getByRole("heading", { name: "New Message" })).toHaveCount(0);

    await app.row("Contract").click();
    await app.reply("f");
    await app.press("Meta+Shift+o");
    await app.type("kim@kite.test,");
    await expect(app.chips("To")).toHaveCount(1);
    await app.settle();
    await app.press("Meta+Enter");

    await app.advance(10_000);
    await app.settle();

    // A forward of `fwd`: the inline image goes back to a cid: part next to the body.
    const fwd = (await sent(gmail)).find((m) => m.message.threadId === s.thread("fwd").threadId);
    const tree = fwd?.mime ?? parseMime("");
    expect(shape(tree)).toBe(
      "multipart/mixed[multipart/related[multipart/alternative[text/plain,text/html],image/png],application/pdf]",
    );
    const png = findPart(tree, "image/png");
    expect(png?.contentId).toBeTruthy();
    expect(htmlOf(tree)).toContain(`cid:${png?.contentId ?? "missing"}`);

    // Encoded headers arrive decoded and identical.
    const plain = await sentWithSubject(gmail, plainSubject);
    expect(plain.headers.subject).toBe(plainSubject);

    // New messages, without and with an attachment.
    expect(shape(plain)).toBe("multipart/related[multipart/alternative[text/plain,text/html]]");
    expect(shape(await sentWithSubject(gmail, "One file"))).toBe(
      "multipart/mixed[multipart/related[multipart/alternative[text/plain,text/html]],application/pdf]",
    );
  });
});
