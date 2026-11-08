// Attachments (SPEC Reading, Drafts, Addressing, Send and undo):
// attach and send, download on demand, offline forwards, the MIME tree.
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";

import type { Route } from "@playwright/test";

import type { App } from "./app.ts";
import type { Control, GmailMessage } from "./control.ts";
import { attachment, files } from "./files.ts";
import { expect, test } from "./fixtures.ts";
import { attachments, findPart, htmlOf, parseMime, shape, type MimePart } from "./mime.ts";
import * as seeds from "./seeds.ts";
import { A, B, C } from "./seeds.ts";

/** The decoded subjects of these messages. */
const subjects = (msgs: GmailMessage[]) => msgs.map((m) => parseMime(m.raw).headers.subject);

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

/** The Subject header as written, folded lines joined, nothing decoded. */
function rawSubject(raw: string): string {
  const [head = ""] = raw.split(/\r?\n\r?\n/, 1);
  return /^subject:(.*)$/im.exec(head.replace(/\r?\n[ \t]+/g, " "))?.[1]?.trim() ?? "";
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

  test("ATT-13 a file Gmail serves inside the message opens and forwards", async ({
    app,
    gmail,
  }) => {
    await seeds.standard(gmail, app);
    await gmail.deliver({
      account: A,
      from: "Nina Notes <nina@notes.test>",
      subject: "Meeting notes",
      text: "Notes attached.",
      attachments: [{ ...attachment(files.notesTxt), servedInline: true }],
    });
    await app.settle();
    await app.row("Meeting notes").click();
    // The received message's chip: the forward below carries one too.
    const chip = app.conversation.getByRole("button", { name: /notes\.txt/ }).first();
    await expect(chip).toContainText("notes.txt");

    // Forwarded before it was ever opened.
    await app.reply("f");
    await app.press("Meta+Shift+o");
    await app.type("kim@kite.test,");
    await expect(app.chips("To")).toHaveCount(1);
    await app.settle();
    await app.press("Meta+Enter");
    await app.advance(10_000);
    await app.settle();
    const forwarded = filesOf(await sentWithSubject(gmail, "Fwd: Meeting notes"));
    expect(forwarded["notes.txt"]?.bytes.equals(files.notesTxt.buffer)).toBe(true);

    await chip.click();
    await app.settle();
    const { openedFiles } = await gmail.platform();
    expect(openedFiles).toHaveLength(1);
    expect(readFileSync(openedFiles[0] ?? "").equals(files.notesTxt.buffer)).toBe(true);
    await app.expectNoErrorToasts();
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

    // Encoded headers: RFC 2047 words on the wire, decoded and identical in Gmail.
    const plain = await sentWithSubject(gmail, plainSubject);
    expect(plain.headers.subject).toBe(plainSubject);
    const wire = (await sent(gmail)).find((m) => m.mime.headers.subject === plainSubject);
    expect(rawSubject(wire?.message.raw ?? "")).toMatch(
      /^[\x20-\x7e]*=\?utf-8\?[bq]\?[\x20-\x7e]*$/i,
    );

    // New messages, without and with an attachment.
    expect(shape(plain)).toBe("multipart/related[multipart/alternative[text/plain,text/html]]");
    expect(shape(await sentWithSubject(gmail, "One file"))).toBe(
      "multipart/mixed[multipart/related[multipart/alternative[text/plain,text/html]],application/pdf]",
    );
  });
  test("ATT-05 a send while a file is still attaching carries the file", async ({ app, gmail }) => {
    await seeds.standard(gmail, app);
    await newMessage(app, "Sent mid-upload", "The file is on its way.");
    const release = await app.park("add_attachment");
    await app.chooseFiles(() => app.press("Meta+Shift+a"), [files.reportPdf]);
    await app.press("Meta+Enter");
    await release();
    await app.settle();
    await app.advance(10_000);
    await app.settle();
    const out = filesOf(await sentWithSubject(gmail, "Sent mid-upload"));
    expect(Object.keys(out)).toEqual(["report.pdf"]);
    expect(out["report.pdf"]?.bytes.equals(files.reportPdf.buffer)).toBe(true);
  });

  test("ATT-06 closing a fresh composer while a file is attaching keeps the draft", async ({
    app,
    gmail,
  }) => {
    await seeds.standard(gmail, app);
    await app.compose();
    const release = await app.park("add_attachment");
    await app.chooseFiles(() => app.press("Meta+Shift+a"), [files.notesTxt]);
    await app.press("Escape", "Escape");
    await expect(app.page.getByRole("heading", { name: "New Message" })).toHaveCount(0);
    await release();
    await app.settle();
    const drafts = (await gmail.gmail(A)).drafts;
    expect(drafts).toHaveLength(1);
    const kept = filesOf(parseMime(drafts[0]?.message.raw ?? ""));
    expect(Object.keys(kept)).toEqual(["notes.txt"]);
  });

  test("ATT-07 a downloaded attachment is quarantined before it opens", async ({ app, gmail }) => {
    await seeds.standard(gmail, app);
    await app.row("Contract").click();
    await app.conversation.getByRole("button", { name: /contract\.pdf/ }).click();
    await app.settle();
    const [opened] = (await gmail.platform()).openedFiles;
    expect(opened).toBeDefined();
    const flag = execFileSync("xattr", ["-p", "com.apple.quarantine", opened ?? ""], {
      encoding: "utf8",
    }).trim();
    const [flags, time, agent, event] = flag.split(";");
    expect(flags).toBe("0081");
    expect(Number.parseInt(time ?? "", 16)).toBe(Math.floor((await gmail.now()) / 1000));
    expect(agent).toBe("Supermonkey");
    expect(event).toMatch(/^[0-9A-F]{8}-[0-9A-F]{4}-4[0-9A-F]{3}-[0-9A-F]{4}-[0-9A-F]{12}$/);
  });

  test("ATT-08 a file over 25 MB is refused before it is read", async ({ app, gmail }) => {
    await seeds.standard(gmail, app);
    await newMessage(app, "Too big", "This one stays small.");
    const added: string[] = [];
    app.page.on("request", (r) => {
      if (r.url().endsWith("/invoke/add_attachment")) added.push(r.url());
    });
    const big = {
      name: "big.bin",
      mimeType: "application/octet-stream",
      buffer: Buffer.alloc(25 * 1024 * 1024 + 1),
    };
    await app.chooseFiles(() => app.press("Meta+Shift+a"), [big]);
    await expect(
      app.toast("Couldn't attach big.bin: attachments can be at most 25 MB"),
    ).toBeVisible();
    await app.settle();
    expect(added, "the file never went to the core").toEqual([]);
    await expect(app.page.locator('[aria-busy="true"]')).toHaveCount(0);

    // A file of exactly 25 MB is taken. Reading and storing it takes a while.
    await app.chooseFiles(
      () => app.press("Meta+Shift+a"),
      [{ ...big, name: "max.bin", buffer: Buffer.alloc(25 * 1024 * 1024) }],
    );
    await expect(app.page.getByRole("button", { name: "Remove max.bin" })).toBeVisible({
      timeout: 20_000,
    });
    await app.settle();
    expect(added).toHaveLength(1);
  });

  test("ATT-09 Cmd+Q while a file is attaching quits once the composer's close is sent", async ({
    app,
    gmail,
  }) => {
    await seeds.standard(gmail, app);
    await app.compose();
    const release = await app.park("add_attachment");
    await app.chooseFiles(() => app.press("Meta+Shift+a"), [files.notesTxt]);
    const quitting = app.quit();
    // Cmd+Q takes the composer off screen at once; its close waits for the file.
    await expect(app.page.getByRole("heading", { name: "New Message" })).toHaveCount(0);
    await release();
    await quitting;
    await app.launch();
    await app.settle();
    const drafts = (await gmail.gmail(A)).drafts;
    expect(drafts).toHaveLength(1);
    const kept = filesOf(parseMime(drafts[0]?.message.raw ?? ""));
    expect(Object.keys(kept)).toEqual(["notes.txt"]);
  });

  test("ATT-10 a send waiting for a file still goes when the composer is closed meanwhile", async ({
    app,
    gmail,
  }) => {
    await seeds.standard(gmail, app);
    const heading = app.page.getByRole("heading", { name: "New Message" });
    await newMessage(app, "Sent after closing", "The file follows.");
    const release = await app.park("add_attachment");
    await app.chooseFiles(() => app.press("Meta+Shift+a"), [files.notesTxt]);
    await app.press("Meta+Enter");
    // The composer stays while the send waits for the file; Esc Esc closes it.
    await expect(heading).toBeVisible();
    await app.press("Escape", "Escape");
    await expect(heading).toHaveCount(0);
    await release();
    await app.settle();
    await expect(app.toast("Sent after closing")).toBeVisible();
    await app.advance(10_000);
    await app.settle();
    const out = filesOf(await sentWithSubject(gmail, "Sent after closing"));
    expect(Object.keys(out)).toEqual(["notes.txt"]);
    expect((await gmail.gmail(A)).drafts).toHaveLength(0);

    // A file that fails to attach sends nothing, and the closed draft is kept.
    await newMessage(app, "Kept after closing", "The file fails.");
    const attach = /\/invoke\/add_attachment$/;
    const parked: Route[] = [];
    await app.page.route(attach, (route) => {
      parked.push(route);
    });
    await app.chooseFiles(() => app.press("Meta+Shift+a"), [files.notesTxt]);
    await app.press("Meta+Enter");
    await app.press("Escape", "Escape");
    await expect(heading).toHaveCount(0);
    await expect.poll(() => parked.length).toBe(1);
    await parked[0]?.fulfill({ status: 400, json: "The file is gone" });
    await app.page.unroute(attach);
    await expect(app.toast("Not sent: a file could not be attached")).toBeVisible();
    await app.settle();
    await app.advance(10_000);
    await app.settle();
    const state = await gmail.gmail(A);
    expect(subjects(state.messages.filter((m) => m.labels.includes("SENT")))).not.toContain(
      "Kept after closing",
    );
    expect(subjects(state.drafts.map((d) => d.message))).toEqual(["Kept after closing"]);
  });

  test("ATT-11 a draft reopened while its close waits for a file keeps the new text and the file", async ({
    app,
    gmail,
  }) => {
    await seeds.standard(gmail, app);
    await app.row("Person 3").click();
    await expect(app.conversationTitle).toHaveText("Person 3");
    await app.reply("r");
    await app.type("Old words.");
    const release = await app.park("add_attachment");
    await app.chooseFiles(() => app.press("Meta+Shift+a"), [files.notesTxt]);
    await app.press("Escape", "Escape");
    await expect(app.editor).toHaveCount(0);

    // `r` resumes the same draft while its close still waits for the file.
    await app.press("r");
    await expect(app.editor).toBeFocused();
    await expect(app.editor).toHaveText("Old words.");
    await app.type(" New words.");
    await release();
    await app.settle();
    await expect(app.page.getByRole("button", { name: "Remove notes.txt" })).toBeVisible();
    await expect(app.editor).toHaveText("Old words. New words.");
    await app.press("Meta+Enter");
    await app.settle();
    await app.advance(10_000);
    await app.settle();

    const reply = await sentWithSubject(gmail, "Re: Person 3");
    expect(htmlOf(reply)).toContain("Old words. New words.");
    expect(Object.keys(filesOf(reply))).toEqual(["notes.txt"]);
    expect((await gmail.gmail(A)).drafts).toHaveLength(0);
  });

  test("ATT-14 a file attached while a send waits for another goes with the send", async ({
    app,
    gmail,
  }) => {
    await seeds.standard(gmail, app);
    const heading = app.page.getByRole("heading", { name: "New Message" });
    await newMessage(app, "Two files", "Both files follow.");
    // Each file's attach is held on its own, so the first can land before the second.
    const attach = /\/invoke\/add_attachment$/;
    const parked = new Map<string, Route>();
    await app.page.route(attach, (route) => {
      const body: unknown = JSON.parse(route.request().postData() ?? "{}");
      const name = typeof body === "object" && body && "filename" in body ? body.filename : "";
      parked.set(String(name), route);
    });
    await app.chooseFiles(() => app.press("Meta+Shift+a"), [files.notesTxt]);
    await expect.poll(() => parked.has("notes.txt")).toBe(true);
    await app.press("Meta+Enter");
    await app.chooseFiles(() => app.press("Meta+Shift+a"), [files.reportPdf]);
    await expect.poll(() => parked.has("report.pdf")).toBe(true);

    // The first file is in; the send still waits for the second, with the composer on screen.
    await parked.get("notes.txt")?.continue();
    await expect(app.page.getByRole("button", { name: "Remove notes.txt" })).toBeVisible();
    await expect(heading).toBeVisible();
    await parked.get("report.pdf")?.continue();
    await app.page.unroute(attach);
    await expect(heading).toHaveCount(0);
    await app.settle();
    await expect(app.toast("Two files")).toBeVisible();
    await app.advance(10_000);
    await app.settle();
    const out = filesOf(await sentWithSubject(gmail, "Two files"));
    expect(Object.keys(out).toSorted()).toEqual(["notes.txt", "report.pdf"]);
    expect((await gmail.gmail(A)).drafts).toHaveLength(0);
  });

  test("ATT-15 discarding while a send waits for a file cancels the send", async ({
    app,
    gmail,
  }) => {
    await seeds.standard(gmail, app);
    const heading = app.page.getByRole("heading", { name: "New Message" });
    await newMessage(app, "Thrown away", "Never sent.");
    const release = await app.park("add_attachment");
    await app.chooseFiles(() => app.press("Meta+Shift+a"), [files.notesTxt]);
    await app.press("Meta+Enter");
    await expect(heading).toBeVisible();
    await app.press("Meta+Shift+Comma");
    await expect(app.toast("Draft discarded")).toBeVisible();
    await expect(heading).toHaveCount(0);

    // The file lands after the discard: nothing is sent, and nothing goes wrong.
    await release();
    await app.expectNoErrorToasts();
    await app.advance(10_000);
    await app.expectNoErrorToasts();
    const state = await gmail.gmail(A);
    expect(subjects(state.messages)).not.toContain("Thrown away");
    expect(state.drafts).toHaveLength(0);
    // The file went with the draft.
    expect(await gmail.draftFolders()).toEqual([]);
  });

  test("ATT-16 the same file can be attached twice", async ({ app, gmail }) => {
    await seeds.standard(gmail, app);
    await newMessage(app, "Same file twice", "Two copies.");
    const removeNotes = app.page.getByRole("button", { name: "Remove notes.txt" });
    await app.chooseFiles(() => app.press("Meta+Shift+a"), [files.notesTxt]);
    await expect(removeNotes).toHaveCount(1);
    // A browser fires no change for a pick equal to the input's value, so the
    // picker forgets each pick. Playwright fires one anyway, so check the value.
    await expect(app.page.locator('input[type="file"]')).toHaveValue("");
    await app.chooseFiles(() => app.press("Meta+Shift+a"), [files.notesTxt]);
    await expect(removeNotes).toHaveCount(2);
    await app.settle();
    await app.press("Meta+Enter");
    await app.settle();
    await app.advance(10_000);
    await app.settle();
    const mime = await sentWithSubject(gmail, "Same file twice");
    expect(attachments(mime).map((p) => p.filename)).toEqual(["notes.txt", "notes.txt"]);
  });

  test("ATT-17 an attachment that can't be opened says why", async ({ app, gmail }) => {
    await seeds.standard(gmail, app);
    await app.row("Contract").click();
    const chip = app.conversation.getByRole("button", { name: /contract\.pdf/ });
    await expect(chip).toBeVisible();
    await app.settle();

    // Offline, a file not yet downloaded can't be had.
    await gmail.offline(true);
    await chip.click();
    await app.settle();
    await expect(app.errorToasts.filter({ hasText: "Couldn't open contract.pdf" })).toBeVisible();
    expect((await gmail.platform()).openedFiles).toEqual([]);

    // Back online, the same click opens it.
    await gmail.offline(false);
    await app.settle();
    await chip.click();
    await app.settle();
    expect((await gmail.platform()).openedFiles).toHaveLength(1);
  });

  test("ATT-18 a file sent inline, as Apple Mail sends it, or an image the body doesn't show is an attachment, and a forward carries it", async ({
    app,
    gmail,
  }) => {
    await seeds.standard(gmail, app);
    // Apple Mail with a plain-text body sends a photo `inline`, with no Content-ID.
    await gmail.deliver({
      account: A,
      from: "Ann Apple <ann@apple.test>",
      subject: "Photo from my phone",
      text: "Here it is.",
      attachments: [{ ...attachment(files.photoPng), inline: true }],
    });
    // HTML that shows its logo but never references the photo.
    await gmail.deliver({
      account: A,
      from: "Carl Chart <carl@chart.test>",
      subject: "Logo and photo",
      html: '<p>The photo is attached.</p><p><img src="cid:logo" alt="logo"></p>',
      attachments: [attachment(files.logoPng, "logo"), attachment(files.photoPng, "unused")],
    });
    // A file sent inline, as Apple Mail sends every file.
    await gmail.deliver({
      account: A,
      from: "Ann Apple <ann@apple.test>",
      subject: "Scans from my phone",
      html: "<p>The scan is attached.</p>",
      attachments: [{ ...attachment(files.reportPdf), inline: true }],
    });
    await app.settle();
    await expect(app.row("Photo from my phone")).toHaveAttribute("data-attachments");
    await expect(app.row("Logo and photo")).toHaveAttribute("data-attachments");
    await expect(app.row("Scans from my phone")).toHaveAttribute("data-attachments");

    // The inline PDF is a chip, and opens.
    await app.row("Scans from my phone").click();
    await expect(app.conversationTitle).toHaveText("Scans from my phone");
    const pdf = app.conversation.getByRole("button", { name: /report\.pdf/ });
    await expect(pdf).toContainText("report.pdf");
    await pdf.click();
    await app.settle();
    const scans = await gmail.platform();
    expect(scans.openedFiles).toHaveLength(1);
    expect(readFileSync(scans.openedFiles[0] ?? "").equals(files.reportPdf.buffer)).toBe(true);
    await app.press("Escape");

    // The logo is in the body, not a chip; the photo is a chip.
    await app.row("Logo and photo").click();
    await expect(app.conversationTitle).toHaveText("Logo and photo");
    await expect(app.messageBody(0).getByRole("img", { name: "logo" })).toHaveAttribute(
      "src",
      /^data:image\/png;base64,/,
    );
    await expect(app.conversation.getByRole("button", { name: /photo\.png/ })).toHaveCount(1);
    await expect(app.conversation.getByRole("button", { name: /logo\.png/ })).toHaveCount(0);
    await app.press("Escape");

    await app.row("Photo from my phone").click();
    await expect(app.conversationTitle).toHaveText("Photo from my phone");
    const chip = app.conversation.getByRole("button", { name: /photo\.png/ });
    await expect(chip).toHaveCount(1);
    await chip.click();
    await app.settle();
    const { openedFiles } = await gmail.platform();
    expect(openedFiles).toHaveLength(2);
    expect(readFileSync(openedFiles[1] ?? "").equals(files.photoPng.buffer)).toBe(true);

    await app.reply("f");
    await app.press("Meta+Shift+o");
    await app.type("kim@kite.test,");
    await expect(app.chips("To")).toHaveCount(1);
    await app.settle();
    await app.press("Meta+Enter");
    await app.advance(10_000);
    await app.settle();
    const forwarded = filesOf(await sentWithSubject(gmail, "Fwd: Photo from my phone"));
    expect(forwarded["photo.png"]?.bytes.equals(files.photoPng.buffer)).toBe(true);
  });

  // The send went ahead for the removed draft once the file was in: Rust
  // refused it, and the draft's "not sent" switched back to the removed account.
  test("ATT-20 removing the account while its composer's send or close waits for a file leaves nothing behind", async ({
    app,
    gmail,
  }) => {
    await seeds.standard(gmail, app, { connect: ["A", "B", "C"] });
    const heading = app.page.getByRole("heading", { name: "New Message" });

    // A send waits for the file when the account on screen is removed.
    await newMessage(app, "Gone with A", "The file is late.");
    let attach = await app.park("add_attachment");
    await app.chooseFiles(() => app.press("Meta+Shift+a"), [files.notesTxt]);
    await app.press("Meta+Enter");
    await expect(heading).toBeVisible();
    await app.removeAccount(A);
    await expect(heading).toHaveCount(0);
    await attach();
    await app.settle();
    await expect(app.account(A)).toHaveCount(0);
    await expect(app.highlighted).toContainText("Bea Blue");
    await expect(heading).toHaveCount(0);
    await app.expectNoErrorToasts();

    // A close waits for the file when its account is removed from another one's screen.
    await newMessage(app, "Gone with B", "The file is late.");
    attach = await app.park("add_attachment");
    await app.chooseFiles(() => app.press("Meta+Shift+a"), [files.notesTxt]);
    await app.press("Escape", "Escape");
    await expect(heading).toHaveCount(0);
    await app.press("Control+2");
    await expect(app.highlighted).toContainText("Cy 1");
    await app.removeAccount(B);
    await attach();
    await app.settle();
    await expect(app.highlighted).toContainText("Cy 1");
    await app.expectNoErrorToasts();
    expect(await gmail.draftFolders()).toEqual([]);
    await app.advance(10_000);
    await app.settle();
    expect(subjects((await gmail.gmail(C)).messages)).toEqual(
      expect.not.arrayContaining(["Gone with A", "Gone with B"]),
    );
  });

  // Its next create named the thread Gmail deleted with the draft, got a 404,
  // and the draft never reached Gmail again.
  test("ATT-21 a file attaching when the draft is deleted in Gmail joins the draft written again", async ({
    app,
    gmail,
  }) => {
    await seeds.standard(gmail, app);
    await newMessage(app, "Deleted elsewhere", "The file follows.");
    await app.settle();
    const [first] = (await gmail.gmail(A)).drafts;
    if (!first) throw new Error("the draft never reached Gmail");
    const created = async () =>
      (await gmail.requests("drafts.create")).filter((r) => r.answer === 200).length;
    const creates = await created();

    const attach = await app.park("add_attachment");
    await app.chooseFiles(() => app.press("Meta+Shift+a"), [files.notesTxt]);
    // The page's attach is parked, so it can't settle: the toast says when the deletion is in.
    await gmail.deleteDraft(A, first.id, { poll: true });
    await expect(app.toast("Draft was sent or deleted elsewhere")).toBeVisible();
    await expect(app.page.getByRole("heading", { name: "New Message" })).toBeVisible();
    await attach();
    await app.settle();
    await expect(app.page.getByRole("button", { name: "Remove notes.txt" })).toBeVisible();
    await app.advance(1_000);
    await app.settle();

    expect(await created()).toBe(creates + 1);
    const drafts = (await gmail.gmail(A)).drafts;
    expect(subjects(drafts.map((d) => d.message))).toEqual(["Deleted elsewhere"]);
    expect(Object.keys(filesOf(parseMime(drafts[0]?.message.raw ?? "")))).toEqual(["notes.txt"]);
    await app.expectNoErrorToasts();
  });

  // The composer left the screen before its close or send ran, and took the
  // typed address with it: the fields that held it were gone by then.
  test("ATT-19 an address typed but not yet a chip goes with a close or send that waits for a file", async ({
    app,
    gmail,
  }) => {
    await seeds.standard(gmail, app);
    const heading = app.page.getByRole("heading", { name: "New Message" });
    const typeCc = async (address: string) => {
      await app.press("Meta+Shift+c");
      await expect(app.field("Cc")).toBeFocused();
      await app.type(address);
    };

    // Closed while the file attaches: the draft keeps the address.
    await newMessage(app, "Closed mid-upload", "Cc is typed.");
    const closing = await app.park("add_attachment");
    await app.chooseFiles(() => app.press("Meta+Shift+a"), [files.notesTxt]);
    await typeCc("cora@cc.test");
    await app.press("Escape", "Escape");
    await expect(heading).toHaveCount(0);
    await closing();
    await app.settle();
    const drafts = (await gmail.gmail(A)).drafts;
    expect(subjects(drafts.map((d) => d.message))).toEqual(["Closed mid-upload"]);
    expect(parseMime(drafts[0]?.message.raw ?? "").headers.cc).toContain("cora@cc.test");

    // Sent while the file attaches, then closed: the send carries the address.
    await newMessage(app, "Sent mid-upload", "Cc is typed.");
    const sending = await app.park("add_attachment");
    await app.chooseFiles(() => app.press("Meta+Shift+a"), [files.notesTxt]);
    await typeCc("dora@cc.test");
    await app.press("Meta+Enter");
    await expect(heading).toBeVisible();
    await app.press("Escape", "Escape");
    await expect(heading).toHaveCount(0);
    await sending();
    await app.settle();
    await app.advance(10_000);
    await app.settle();
    const sentMime = await sentWithSubject(gmail, "Sent mid-upload");
    expect(sentMime.headers.cc).toContain("dora@cc.test");
    expect(Object.keys(filesOf(sentMime))).toEqual(["notes.txt"]);
  });
});
