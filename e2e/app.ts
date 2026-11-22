import { expect, type Locator, type Page, type Route } from "@playwright/test";

import type { Control, GmailState } from "./control.ts";
import type { FileFixture } from "./files.ts";

type PagePending = {
  invokes: number;
  signIns: number;
  timers: number;
  buffered: number;
  rev: number;
  ack: number;
};

declare global {
  interface Window {
    e2eRowsAdded?: string[];
    e2eErrorToasts?: string[];
  }
}

const PAGE_PENDING = `(async () => {
  if (!window.__sm) return null;
  await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
  return window.__sm.pending();
})()`;

const NAVIGATING: PagePending = { invokes: 1, signIns: 0, timers: 0, buffered: 0, rev: 0, ack: 0 };

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

const SHIFTED = /^[~!@#$%^&*()_+{}|:"<>?]$/;

const POLL_MS = 45_000;

export class App {
  readonly page: Page;
  readonly gmail: Control;
  readonly popups: string[] = [];
  private clockInstalled = false;
  private toastLogInstalled = false;

  constructor(page: Page, gmail: Control) {
    this.page = page;
    this.gmail = gmail;
    page.on("popup", (popup) => this.popups.push(popup.url()));
  }

  async open(opts: { timeout?: number } = {}): Promise<void> {
    await this.installClock();
    await this.installToastLog();
    await this.routeHostile();
    await this.gmail.start();
    await this.load(opts.timeout);
  }

  async quit(): Promise<void> {
    await this.gmail.stop({ graceful: true });
    await this.page.goto("about:blank");
  }

  async crash(): Promise<void> {
    await this.gmail.stop({ graceful: false });
    await this.page.goto("about:blank");
  }

  async launch(): Promise<void> {
    await this.gmail.start();
    await this.load();
  }

  async restart(whileDown?: () => Promise<void>): Promise<void> {
    await this.quit();
    await whileDown?.();
    await this.launch();
  }

  async crashAndRelaunch(whileDown?: () => Promise<void>): Promise<void> {
    await this.crash();
    await whileDown?.();
    await this.launch();
  }

  async reload(): Promise<void> {
    await this.page.reload();
    await this.ready();
    await this.settle();
  }

  private async load(timeout?: number): Promise<void> {
    await this.page.goto(this.gmail.url);
    await this.ready();
    await this.settle(timeout);
  }

  private async ready(): Promise<void> {
    await expect(this.page.getByRole("navigation").or(this.firstRunButton)).toBeVisible();
  }

  private async installClock(): Promise<void> {
    if (this.clockInstalled) return;
    await this.page.addInitScript(() => {
      addEventListener("DOMContentLoaded", () => {
        const style = document.createElement("style");
        style.textContent = '[data-sonner-toast][data-removed="true"]{pointer-events:none}';
        document.head.append(style);
      });
    });
    const now = await this.gmail.now();
    await this.page.clock.install({ time: now });
    await this.page.clock.setFixedTime(now);
    this.clockInstalled = true;
  }

  private async installToastLog(): Promise<void> {
    if (this.toastLogInstalled) return;
    await this.page.addInitScript(() => {
      if (window !== window.top) return;
      const seen: string[] = [];
      window.e2eErrorToasts = seen;
      const record = (el: Element) => {
        if (el.matches('[data-sonner-toast][data-type="error"]:not([data-e2e-seen])')) {
          el.setAttribute("data-e2e-seen", "");
          seen.push(el.textContent);
        }
      };
      new MutationObserver((records) => {
        for (const r of records) {
          if (r.target instanceof Element) record(r.target);
          for (const node of Array.from(r.addedNodes)) {
            if (!(node instanceof Element)) continue;
            record(node);
            node.querySelectorAll("[data-sonner-toast]").forEach(record);
          }
        }
      }).observe(document, {
        childList: true,
        subtree: true,
        attributes: true,
        attributeFilter: ["data-type"],
      });
    });
    this.toastLogInstalled = true;
  }

  private async routeHostile(): Promise<void> {
    await this.page.context().route(/^https?:\/\/evil\.test\//, async (route) => {
      const url = new URL(route.request().url());
      await route.fulfill({
        response: await route.fetch({ url: `${this.gmail.url}/sink/evil.test${url.pathname}` }),
      });
    });
  }

  async settle(timeout = 10_000): Promise<void> {
    await this.settleRound(Date.now() + timeout, "", timeout);
  }

  private async settleRound(deadline: number, previous: string, timeout: number): Promise<void> {
    const core = await this.gmail.idle(timeout);
    const page = await this.pagePending();
    const idle =
      page === null ||
      (page.invokes === 0 &&
        page.timers === 0 &&
        page.buffered === 0 &&
        page.signIns <= core.signIns &&
        page.rev >= core.rev &&
        page.ack >= core.ack);
    const seen = idle ? JSON.stringify([core, page]) : "";
    if (idle && seen === previous) return;
    if (Date.now() > deadline) {
      throw new Error(
        `settle: still busy. core ${JSON.stringify(core)}, page ${JSON.stringify(page)}`,
      );
    }
    if (!idle) await sleep(5);
    await this.settleRound(deadline, seen, timeout);
  }

  private async pagePending(): Promise<PagePending | null> {
    try {
      return await this.page.evaluate<PagePending | null>(PAGE_PENDING);
    } catch (e) {
      if (e instanceof Error && /context|navigat|Target/i.test(e.message)) return NAVIGATING;
      throw e;
    }
  }

  now(): Promise<number> {
    return this.gmail.now();
  }

  async advance(ms: number, opts: { stepwise?: boolean } = {}): Promise<void> {
    await this.settle();
    const { now } = await this.gmail.advanceServerClock(ms, opts);
    await this.page.clock.setFixedTime(now);
    if (opts.stepwise) await this.page.clock.runFor(ms);
    else await this.page.clock.fastForward(ms);
  }

  async advanceTo(time: number, opts: { stepwise?: boolean } = {}): Promise<void> {
    const now = await this.gmail.now();
    if (time < now) throw new Error(`advanceTo: ${new Date(time).toISOString()} is in the past`);
    await this.advance(time - now, opts);
  }

  async sleep(ms: number): Promise<void> {
    await this.settle();
    const { now } = await this.gmail.jumpServerClock(ms);
    await this.page.clock.setFixedTime(now);
  }

  async sleepAndWake(ms: number): Promise<void> {
    await this.sleep(ms);
    await this.advance(POLL_MS);
  }

  async focus(focused: boolean): Promise<{ account: string; thread: string | null } | null> {
    await this.page.evaluate((f) => {
      if (f) Reflect.deleteProperty(document, "hasFocus");
      else Object.defineProperty(document, "hasFocus", { value: () => false, configurable: true });
      window.dispatchEvent(new Event(f ? "focus" : "blur"));
    }, focused);
    const r = await this.gmail.focus(focused);
    return r?.openThread ?? null;
  }

  async park(
    cmd: string,
    which: (body: string) => boolean = () => true,
  ): Promise<() => Promise<void>> {
    const parked: Route[] = [];
    let open = false;
    const url = new RegExp(`/invoke/${cmd}$`);
    await this.page.route(url, async (route) => {
      if (open || !which(route.request().postData() ?? "")) await route.continue();
      else parked.push(route);
    });
    return async () => {
      open = true;
      await parked.reduce((prev, r) => prev.then(() => r.continue()), Promise.resolve());
      await this.page.unroute(url);
    };
  }

  async press(...keys: string[]): Promise<void> {
    const keyboard = this.page.keyboard;
    const one = async (k: string) => {
      if (!SHIFTED.test(k)) return keyboard.press(k);
      await keyboard.down("Shift");
      await keyboard.press(k);
      await keyboard.up("Shift");
    };
    await keys.reduce((prev, k) => prev.then(() => one(k)), Promise.resolve());
  }

  async type(text: string): Promise<void> {
    await this.page.keyboard.type(text);
  }

  async paste(text: string): Promise<void> {
    await this.page.evaluate((t) => {
      const data = new DataTransfer();
      data.setData("text/plain", t);
      const target = document.activeElement ?? document.body;
      target.dispatchEvent(
        new ClipboardEvent("paste", { clipboardData: data, bubbles: true, cancelable: true }),
      );
    }, text);
  }

  async chooseFiles(open: () => Promise<void>, chosen: FileFixture[]): Promise<void> {
    const chooser = this.page.waitForEvent("filechooser");
    await open();
    await (
      await chooser
    ).setFiles(chosen.map((f) => ({ name: f.name, mimeType: f.mimeType, buffer: f.buffer })));
  }

  async watchRows(): Promise<{ added: () => Promise<string[]> }> {
    await this.page.evaluate(() => {
      const added: string[] = [];
      window.e2eRowsAdded = added;
      new MutationObserver((records) => {
        for (const node of records.flatMap((r) => Array.from(r.addedNodes))) {
          if (!(node instanceof Element)) continue;
          const rows = node.matches("[data-row]")
            ? [node]
            : Array.from(node.querySelectorAll("[data-row]"));
          added.push(...rows.map((r) => r.getAttribute("data-row") ?? ""));
        }
      }).observe(document.body, { childList: true, subtree: true });
    });
    return { added: () => this.page.evaluate(() => window.e2eRowsAdded ?? []) };
  }

  get list(): Locator {
    return this.page.getByRole("listbox", { name: "Conversations" });
  }

  get rows(): Locator {
    return this.list.getByRole("option");
  }

  row(text: string | RegExp): Locator {
    return this.rows.filter({ hasText: text });
  }

  async rowKey(text: string | RegExp): Promise<string> {
    const key = await this.row(text).getAttribute("data-row");
    if (key === null) throw new Error(`row ${String(text)} has no key`);
    return key;
  }

  get highlighted(): Locator {
    return this.list.locator('[role="option"][aria-selected="true"]');
  }

  async highlight(text: string | RegExp, opts: { settle?: boolean } = {}): Promise<void> {
    const target = this.row(text);
    await expect(target).toHaveCount(1);
    await expect(target).toBeVisible();
    await expect(this.highlighted).toHaveCount(1);
    if (opts.settle) await this.settle();
    const keys = await this.rows.evaluateAll((els) => els.map((el) => el.getAttribute("data-row")));
    const to = keys.indexOf(await target.getAttribute("data-row"));
    const from = keys.indexOf(await this.highlighted.getAttribute("data-row"));
    const delta = to - from;
    await this.press(...Array.from({ length: Math.abs(delta) }, () => (delta > 0 ? "j" : "k")));
    await expect(this.highlighted).toContainText(text);
  }

  async selectFrom(first: string, count: number): Promise<void> {
    await this.highlight(first);
    await this.press("x", ...Array.from({ length: count - 1 }, () => "Shift+J"));
    await expect(this.selected).toHaveCount(count);
  }

  get selected(): Locator {
    return this.list.locator("[data-row][data-selected]");
  }

  get unread(): Locator {
    return this.list.locator("[data-row][data-unread]");
  }

  async subjects(): Promise<string[]> {
    return this.rows.evaluateAll((els) =>
      els.map((el) => el.querySelector(".flex-1 > .truncate")?.firstChild?.textContent ?? ""),
    );
  }

  splitTitles(): Locator {
    return this.page.getByRole("toolbar", { name: "Splits" }).getByRole("button");
  }

  async splitHeader(): Promise<string> {
    const text = await this.page.getByRole("toolbar", { name: "Splits" }).innerText();
    return text.replace(/\s+/g, " ").trim();
  }

  get activeSplit(): Locator {
    return this.splitTitles().and(this.page.locator('[aria-current="true"]'));
  }

  section(name: string): Locator {
    return this.list.getByRole("heading", { name: new RegExp(`^${name}( |$)`) });
  }

  async sections(): Promise<{ name: string; subjects: string[] }[]> {
    return this.list.evaluate((list) => {
      const out: { name: string; subjects: string[] }[] = [];
      const items = Array.from(list.querySelectorAll("[data-split], [data-row]"));
      const tops = items.map((el) => el.getBoundingClientRect().top);
      const order = items
        .map((el, i) => ({ el, top: tops[i] ?? 0 }))
        .toSorted((a, b) => a.top - b.top);
      for (const { el } of order) {
        if (el.hasAttribute("data-split")) {
          out.push({ name: el.querySelector("span")?.textContent ?? "", subjects: [] });
        } else {
          if (out.length === 0) out.push({ name: "", subjects: [] });
          const subject = el.querySelector(".flex-1 > .truncate")?.firstChild?.textContent ?? "";
          out.at(-1)?.subjects.push(subject);
        }
      }
      return out;
    });
  }

  get conversation(): Locator {
    return this.page.getByRole("main");
  }

  get conversationTitle(): Locator {
    return this.conversation.getByRole("heading", { level: 1 });
  }

  get messages(): Locator {
    return this.conversation.locator("[data-message]");
  }

  get selectedMessage(): Locator {
    return this.conversation.locator('[data-message][aria-current="true"]');
  }

  messageBody(nth: number) {
    return this.conversation.locator("iframe[data-frame]").nth(nth).contentFrame();
  }

  get folds(): Locator {
    return this.conversation.getByRole("button", { name: /older messages/ });
  }

  async openThread(subject: string): Promise<void> {
    await this.row(subject).first().click();
    await expect(this.conversationTitle).toContainText(subject);
  }

  async openAndSettle(subject: string): Promise<void> {
    await this.row(subject).first().click();
    await expect(this.conversationTitle).toHaveText(subject);
    await this.settle();
  }

  get viewTitle(): Locator {
    return this.page.locator("div:has(> button[title='Search (/)'])");
  }

  get palette(): Locator {
    return this.page.getByRole("dialog").filter({ has: this.page.getByRole("combobox") });
  }

  async command(name: string): Promise<void> {
    await this.press("Meta+k");
    await expect(this.palette).toBeVisible();
    await this.type(name);
    await expect(this.palette.locator("[cmdk-item]").first()).toContainText(name);
    await this.press("Enter");
  }

  async pick(key: string, query = ""): Promise<void> {
    await this.press(key);
    await expect(this.palette).toBeVisible();
    if (query) await this.type(query);
    await expect(this.palette.locator("[cmdk-item]").first()).toBeVisible();
    await this.press("Enter");
    await expect(this.palette).toHaveCount(0);
  }

  async goLabel(name: string): Promise<void> {
    await this.press("g", "l");
    await expect(this.palette).toBeVisible();
    await this.palette.getByRole("option", { name, exact: true }).click();
    await expect(this.palette).toHaveCount(0);
  }

  sheet(title: string | RegExp): Locator {
    return this.page.getByRole("dialog", { name: title });
  }

  get hintBar(): Locator {
    return this.page.locator("[data-hintbar]");
  }

  get toasts(): Locator {
    return this.page.locator('[data-sonner-toast]:not([data-removed="true"])');
  }

  toast(text: string | RegExp): Locator {
    return this.toasts.filter({ hasText: text });
  }

  get errorToasts(): Locator {
    return this.page.locator('[data-sonner-toast][data-type="error"]:not([data-removed="true"])');
  }

  async errorToastsShown(since = 0): Promise<string[]> {
    const shown = await this.page.evaluate(() => window.e2eErrorToasts ?? null);
    if (shown === null) throw new Error("the page has no error toast log");
    return shown.slice(since);
  }

  async expectNoErrorToasts(since = 0): Promise<void> {
    await this.settle();
    expect(await this.errorToastsShown(since), "error toasts shown").toEqual([]);
  }

  async compose(): Promise<void> {
    await this.press("c");
    await expect(this.page.getByRole("heading", { name: "New Message" })).toBeVisible();
    await this.settle();
    await expect(this.field("To")).toBeFocused();
  }

  async reply(key: "r" | "Enter" | "f" = "r"): Promise<void> {
    await this.press(key);
    await expect(this.editor).toBeFocused();
    await this.settle();
  }

  async closeComposer(): Promise<void> {
    await this.press("Escape", "Escape");
    await expect(this.editor).toHaveCount(0);
    await this.settle();
  }

  async send(): Promise<void> {
    await this.settle();
    await this.press("Meta+Enter");
    await expect(this.editor).toHaveCount(0);
    await this.advance(10_000);
    await this.settle();
  }

  field(label: "To" | "Cc" | "Bcc"): Locator {
    return this.page.getByRole("combobox", { name: label, exact: true });
  }

  get fromButton(): Locator {
    return this.page
      .locator("div")
      .filter({ has: this.page.getByText("From", { exact: true }) })
      .last()
      .getByRole("button");
  }

  get draftStubs(): Locator {
    return this.conversation.getByRole("button", { name: /^Draft/ });
  }

  get editor(): Locator {
    return this.page.locator(".ProseMirror");
  }

  get subject(): Locator {
    return this.page.getByPlaceholder("Subject");
  }

  chips(label: "To" | "Cc" | "Bcc"): Locator {
    return this.field(label).locator("xpath=..").locator("[data-chip]");
  }

  async chipAddresses(label: "To" | "Cc" | "Bcc"): Promise<string[]> {
    return this.chips(label).evaluateAll((els) =>
      els.map((el) => el.getAttribute("data-chip") ?? ""),
    );
  }

  invalidChips(label: "To" | "Cc" | "Bcc"): Locator {
    return this.chips(label).and(this.page.locator('[aria-invalid="true"]'));
  }

  suggestions(label: "To" | "Cc" | "Bcc" = "To"): Locator {
    return this.page.locator(`#${label}-suggestions`).getByRole("option");
  }

  get firstRunButton(): Locator {
    return this.page.getByRole("button", { name: "Add a Gmail account" });
  }

  account(email: string): Locator {
    return this.page
      .getByRole("navigation")
      .getByRole("button", { description: new RegExp(email.replace(/[.+]/g, "\\$&")) });
  }

  syncMarker(email: string): Locator {
    return this.account(email).locator('[data-marker="retrying"]');
  }

  signInMarker(email: string): Locator {
    return this.account(email).locator('[data-marker="signin"]');
  }

  async avatarCount(email: string): Promise<number> {
    const count = this.account(email).locator("span.tabular");
    return (await count.count()) === 0 ? 0 : Number(await count.innerText());
  }

  settingsBlock(email: string): Locator {
    return this.sheet("Settings")
      .locator("div")
      .filter({ has: this.page.getByText(email, { exact: true }) })
      .filter({ has: this.page.getByRole("button", { name: "Remove account" }) })
      .last();
  }

  async removeAccount(email: string): Promise<void> {
    await this.press("Meta+,");
    const settings = this.sheet("Settings");
    await this.settingsBlock(email).getByRole("button", { name: "Remove account" }).click();
    await settings.getByRole("button", { name: "Remove", exact: true }).click();
    await expect(this.account(email)).toHaveCount(0);
    await this.press("Escape");
    await expect(settings).toHaveCount(0);
  }

  async pollAndSettle(email: string): Promise<void> {
    const polls = async () =>
      (await this.gmail.requests("history.list", { account: email })).filter(
        (r) => r.answer === 200,
      ).length;
    const before = await polls();
    await expect
      .poll(async () => {
        await this.gmail.poll();
        await this.settle();
        return polls();
      })
      .toBeGreaterThan(before);
  }

  async eventually(
    email: string,
    check: (state: GmailState) => boolean,
    message = "Gmail state",
  ): Promise<void> {
    await expect
      .poll(async () => check(await this.gmail.gmail(email)), { message, timeout: 10_000 })
      .toBe(true);
  }
}
