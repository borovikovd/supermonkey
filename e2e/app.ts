// The app as a user sees it: page helpers shared by every spec. Prefer
// role- and label-based locators; add helpers here rather than repeating
// selectors across specs.

import { expect, type Locator, type Page, type Route } from "@playwright/test";

import type { Control, GmailState } from "./control.ts";
import type { FileFixture } from "./files.ts";

/** The page's pending work, from `window.__sm.pending()` (src/ipc/bridge.ts). */
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
    /** Set by `App.watchRows`. */
    e2eRowsAdded?: string[];
    /** Texts of the error toasts shown since the page loaded, in order (`App.installToastLog`). */
    e2eErrorToasts?: string[];
  }
}

/**
 * Reads the page's pending work after two frames, so work a frame defers is
 * counted; null when no app page is loaded. A string, because the hook is
 * `window.__sm` (src/main.tsx).
 */
const PAGE_PENDING = `(async () => {
  if (!window.__sm) return null;
  await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
  return window.__sm.pending();
})()`;

/** What `settle()` reads from a page that is mid-navigation: busy. */
const NAVIGATING: PagePending = { invokes: 1, signIns: 0, timers: 0, buffered: 0, rev: 0, ack: 0 };

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Symbols a US keyboard types with Shift held. */
const SHIFTED = /^[~!@#$%^&*()_+{}|:"<>?]$/;

/** The Mac's poll interval: after a sleep, the poll timer notices the gap. */
const POLL_MS = 45_000;

/**
 * How a case asserts:
 * - What the user sees first; then `settle()` and read Gmail with
 *   `gmail.gmail(email)`.
 * - "Before Gmail answers": park the request with `gmail.hold`, assert, then
 *   release it. Never sleep.
 * - Time moves only through `advance`; nothing waits on the wall clock.
 * - A negative assertion (no request, no toast, nothing changed) needs
 *   `settle()` first, or it passes vacuously. "No error toast" is
 *   `expectNoErrorToasts()`: a toast leaves by itself 4 s after it shows,
 *   so a count of the toasts on screen can miss one.
 * - A label is on a thread in Gmail when a message outside trash and spam
 *   carries it (SPEC Views).
 */
export class App {
  readonly page: Page;
  readonly gmail: Control;
  /** URLs of every popup window the page opened (links must never open one). */
  readonly popups: string[] = [];
  private clockInstalled = false;
  private toastLogInstalled = false;

  constructor(page: Page, gmail: Control) {
    this.page = page;
    this.gmail = gmail;
    page.on("popup", (popup) => this.popups.push(popup.url()));
  }

  // ---------------------------------------------------------------- lifecycle

  /**
   * Starts the engine, loads the UI, and settles (the connect sync is done).
   * The browser clock is pinned to app time; `advance` moves both.
   */
  async open(opts: { timeout?: number } = {}): Promise<void> {
    await this.installClock();
    await this.installToastLog();
    await this.routeHostile();
    await this.gmail.start();
    await this.load(opts.timeout);
  }

  /** Quits as Cmd+Q does: an open composer closes as a second Esc would, then the core stops. */
  async quit(): Promise<void> {
    await this.gmail.stop({ graceful: true });
    await this.page.goto("about:blank");
  }

  /** Kills the core: no composer close. */
  async crash(): Promise<void> {
    await this.gmail.stop({ graceful: false });
    await this.page.goto("about:blank");
  }

  /** Relaunches the core on the same database and fake Gmail, then loads the UI. */
  async launch(): Promise<void> {
    await this.gmail.start();
    await this.load();
  }

  /**
   * Quits and relaunches. `whileDown` runs in between: change Gmail, move the
   * clock (`advance` works while down), add holds.
   */
  async restart(whileDown?: () => Promise<void>): Promise<void> {
    await this.quit();
    await whileDown?.();
    await this.launch();
  }

  /** Crashes and relaunches; see `restart`. */
  async crashAndRelaunch(whileDown?: () => Promise<void>): Promise<void> {
    await this.crash();
    await whileDown?.();
    await this.launch();
  }

  /** Reloads the webview against the live core. */
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

  /** The app has rendered: the rail, or the first-run screen. */
  private async ready(): Promise<void> {
    await expect(this.page.getByRole("navigation").or(this.firstRunButton)).toBeVisible();
  }

  private async installClock(): Promise<void> {
    if (this.clockInstalled) return;
    // A dismissed toast stays in the DOM for its 200 ms exit animation, and a
    // test clicks sooner than that. It must not catch the click; in the app
    // it is gone before a person could click.
    await this.page.addInitScript(() => {
      addEventListener("DOMContentLoaded", () => {
        const style = document.createElement("style");
        style.textContent = '[data-sonner-toast][data-removed="true"]{pointer-events:none}';
        document.head.append(style);
      });
    });
    const now = await this.gmail.now();
    await this.page.clock.install({ time: now });
    // Date.now() stands still with the core's clock until a test moves it
    // (`advance`). Page timers still run in real time (`setFixedTime` keeps
    // them running): a toast leaves by itself 4 s after it shows, and the
    // autosave runs 500 ms after typing. `advance` fires the ones due at once;
    // `page.clock.pauseAt` stops them.
    await this.page.clock.setFixedTime(now);
    this.clockInstalled = true;
  }

  /**
   * Records the text of every error toast the page shows, from its first
   * frame, so a test sees one that Sonner has already dismissed
   * (`errorToastsShown`, `expectNoErrorToasts`).
   */
  private async installToastLog(): Promise<void> {
    if (this.toastLogInstalled) return;
    await this.page.addInitScript(() => {
      if (window !== window.top) return;
      const seen: string[] = [];
      window.e2eErrorToasts = seen;
      // Each toast once: when it is added, or when it turns into an error toast.
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

  /** `evil.test` goes to the server's sink, so a hit is logged instead of failing DNS. */
  private async routeHostile(): Promise<void> {
    await this.page.context().route(/^https?:\/\/evil\.test\//, async (route) => {
      const url = new URL(route.request().url());
      await route.fulfill({
        response: await route.fetch({ url: `${this.gmail.url}/sink/evil.test${url.pathname}` }),
      });
    });
  }

  // ---------------------------------------------------------------- settle and time

  /**
   * Waits until the core is idle (no Gmail request in flight except held
   * ones, queues empty or blocked, writer idle) and the page has applied the
   * latest channel message and has nothing queued for the core. Call it
   * before negative assertions and before reading Gmail state.
   */
  async settle(timeout = 10_000): Promise<void> {
    await this.settleRound(Date.now() + timeout, "", timeout);
  }

  /** One look at the core and the page; done when two looks in a row see the same idle state. */
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
      // A navigation destroyed the context; the next round asks the new page.
      if (e instanceof Error && /context|navigat|Target/i.test(e.message)) return NAVIGATING;
      throw e;
    }
  }

  /** App time, Unix ms. */
  now(): Promise<number> {
    return this.gmail.now();
  }

  /**
   * Moves app time forward in the core and the browser together. Every timer
   * due by then fires once, at the new time (Playwright's `fastForward`). With
   * `stepwise`, timers fire one by one at their own times and the core settles
   * between them, so a back-off runs every retry it would in that time.
   * Settles first, so what the user did so far happens before time moves;
   * returns once the core has settled again. A Gmail search, for one, runs
   * only after `advance(300)` (SPEC Search).
   */
  async advance(ms: number, opts: { stepwise?: boolean } = {}): Promise<void> {
    await this.settle();
    const { now } = await this.gmail.advanceServerClock(ms, opts);
    await this.page.clock.setFixedTime(now);
    if (opts.stepwise) await this.page.clock.runFor(ms);
    else await this.page.clock.fastForward(ms);
  }

  /** Advances to a time, e.g. `advanceTo(dates.at("2026-11-02T18:00"))`. */
  async advanceTo(time: number, opts: { stepwise?: boolean } = {}): Promise<void> {
    const now = await this.gmail.now();
    if (time < now) throw new Error(`advanceTo: ${new Date(time).toISOString()} is in the past`);
    await this.advance(time - now, opts);
  }

  /**
   * The Mac sleeps for `ms`: the wall clock jumps and no timer fires. Timers
   * resume where they were, as monotonic timers do after a real sleep.
   */
  async sleep(ms: number): Promise<void> {
    await this.settle();
    const { now } = await this.gmail.jumpServerClock(ms);
    await this.page.clock.setFixedTime(now);
  }

  /**
   * The Mac sleeps for `ms`, then wakes and stays awake until the next poll
   * timer fires, which notices the gap (SPEC Sync: wake from sleep).
   */
  async sleepAndWake(ms: number): Promise<void> {
    await this.sleep(ms);
    await this.advance(POLL_MS);
  }

  // ---------------------------------------------------------------- window

  /**
   * The window gains or loses focus: the core's focus rules, and the page's
   * `document.hasFocus()`. Focus soon after a background notification counts
   * as a click on it (the page then opens it); its target is returned.
   */
  async focus(focused: boolean): Promise<{ account: string; thread: string | null } | null> {
    await this.page.evaluate((f) => {
      if (f) Reflect.deleteProperty(document, "hasFocus");
      else Object.defineProperty(document, "hasFocus", { value: () => false, configurable: true });
      window.dispatchEvent(new Event(f ? "focus" : "blur"));
    }, focused);
    const r = await this.gmail.focus(focused);
    return r?.openThread ?? null;
  }

  /**
   * Parks the page's `/invoke/<cmd>` requests whose JSON body passes `which`
   * (all by default) until the returned release, which sends them on in
   * order and lets later ones through. `settle()` waits on a parked invoke,
   * so release first. To keep page timers (the autosave, prefetch) from
   * firing meanwhile, `page.clock.pauseAt(await app.now())`; before
   * `settle()`, which needs animation frames, `page.clock.resume()` and
   * `page.clock.setFixedTime(await app.now())`.
   */
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

  /**
   * Presses keys in order, e.g. `press("g", "i")` or `press("Meta+k")`. A
   * symbol typed with Shift on a US keyboard (`#`, `!`, `?`) goes down with
   * Shift, as a keyboard sends it: Shift's own keydown first.
   */
  async press(...keys: string[]): Promise<void> {
    const keyboard = this.page.keyboard;
    const one = async (k: string) => {
      if (!SHIFTED.test(k)) return keyboard.press(k);
      await keyboard.down("Shift");
      await keyboard.press(k);
      await keyboard.up("Shift");
    };
    // In order: each key must land before the next.
    await keys.reduce((prev, k) => prev.then(() => one(k)), Promise.resolve());
  }

  async type(text: string): Promise<void> {
    await this.page.keyboard.type(text);
  }

  /** Pastes text into the focused element, as Cmd+V would. */
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

  /** Runs `open` (a key or click that opens the file chooser) and picks `chosen`. */
  async chooseFiles(open: () => Promise<void>, chosen: FileFixture[]): Promise<void> {
    const chooser = this.page.waitForEvent("filechooser");
    await open();
    await (
      await chooser
    ).setFiles(chosen.map((f) => ({ name: f.name, mimeType: f.mimeType, buffer: f.buffer })));
  }

  /**
   * Starts recording every list row added to the DOM. `added()` returns
   * their keys (`data-row`) in order; see `rowKey`.
   */
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

  // ---------------------------------------------------------------- list

  get list(): Locator {
    return this.page.getByRole("listbox", { name: "Conversations" });
  }

  get rows(): Locator {
    return this.list.getByRole("option");
  }

  /** The row whose text contains `text` (subject, sender, or snippet). */
  row(text: string | RegExp): Locator {
    return this.rows.filter({ hasText: text });
  }

  /** A row's key (`data-row`, the local thread id), for `watchRows`. */
  async rowKey(text: string | RegExp): Promise<string> {
    const key = await this.row(text).getAttribute("data-row");
    if (key === null) throw new Error(`row ${String(text)} has no key`);
    return key;
  }

  get highlighted(): Locator {
    return this.list.locator('[role="option"][aria-selected="true"]');
  }

  /** Rows selected with `x` / `Shift+J`. */
  get selected(): Locator {
    return this.list.locator("[data-row][data-selected]");
  }

  /** Rows with the unread mark. */
  get unread(): Locator {
    return this.list.locator("[data-row][data-unread]");
  }

  /** Subjects of the rendered rows, top to bottom. */
  async subjects(): Promise<string[]> {
    return this.rows.evaluateAll((els) =>
      els.map((el) => el.querySelector(".flex-1 > .truncate")?.firstChild?.textContent ?? ""),
    );
  }

  /** The split names in the Inbox title, in order (with their counts as text). */
  splitTitles(): Locator {
    return this.page.getByRole("toolbar", { name: "Splits" }).getByRole("button");
  }

  /** The Inbox title as text, e.g. `Important 16 · Other 7`. */
  async splitHeader(): Promise<string> {
    const text = await this.page.getByRole("toolbar", { name: "Splits" }).innerText();
    return text.replace(/\s+/g, " ").trim();
  }

  /** The split name holding the highlight. */
  get activeSplit(): Locator {
    return this.splitTitles().and(this.page.locator('[aria-current="true"]'));
  }

  /** A split's section header in the list (named `Important 16`, or just the name when empty). */
  section(name: string): Locator {
    return this.list.getByRole("heading", { name: new RegExp(`^${name}( |$)`) });
  }

  /**
   * The rendered list grouped by split section: each header's name with the
   * subjects of the rows under it. A flat list is one group with name "".
   */
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

  // ---------------------------------------------------------------- conversation

  get conversation(): Locator {
    return this.page.getByRole("main");
  }

  get conversationTitle(): Locator {
    return this.conversation.getByRole("heading", { level: 1 });
  }

  /** Messages in the open conversation (expanded and collapsed), oldest first. */
  get messages(): Locator {
    return this.conversation.locator("[data-message]");
  }

  /** The selected message (the accent bar). */
  get selectedMessage(): Locator {
    return this.conversation.locator('[data-message][aria-current="true"]');
  }

  /** A message body frame by its position among expanded messages. */
  messageBody(nth: number) {
    return this.conversation.locator("iframe[data-frame]").nth(nth).contentFrame();
  }

  /** A fold line ("N older messages"). */
  get folds(): Locator {
    return this.conversation.getByRole("button", { name: /older messages/ });
  }

  // ---------------------------------------------------------------- overlays

  get palette(): Locator {
    return this.page.getByRole("dialog").filter({ has: this.page.getByRole("combobox") });
  }

  sheet(title: string | RegExp): Locator {
    return this.page.getByRole("dialog", { name: title });
  }

  /** The hint bar along the bottom (`data-hintbar`). */
  get hintBar(): Locator {
    return this.page.locator("[data-hintbar]");
  }

  /** Toasts on screen. A dismissed toast stays in the DOM for its exit animation; it is not counted. */
  get toasts(): Locator {
    return this.page.locator('[data-sonner-toast]:not([data-removed="true"])');
  }

  toast(text: string | RegExp): Locator {
    return this.toasts.filter({ hasText: text });
  }

  /**
   * Error toasts on screen (Sonner's error style). One leaves by itself 4 s
   * after it shows: to check that none showed, use `expectNoErrorToasts`.
   */
  get errorToasts(): Locator {
    return this.page.locator('[data-sonner-toast][data-type="error"]:not([data-removed="true"])');
  }

  /**
   * The texts of the error toasts this page has shown since it loaded, in
   * order, dismissed ones included; from the `since`-th on, a count this
   * returned earlier. A reload or relaunch starts a new page.
   */
  async errorToastsShown(since = 0): Promise<string[]> {
    const shown = await this.page.evaluate(() => window.e2eErrorToasts ?? null);
    if (shown === null) throw new Error("the page has no error toast log");
    return shown.slice(since);
  }

  /**
   * Settles, then fails if this page has shown an error toast since it
   * loaded, or since `since` (see `errorToastsShown`), even one that has
   * already left.
   */
  async expectNoErrorToasts(since = 0): Promise<void> {
    await this.settle();
    expect(await this.errorToastsShown(since), "error toasts shown").toEqual([]);
  }

  // ---------------------------------------------------------------- composer

  /**
   * Presses `c` and waits until the new message is ready to type in: To has
   * focus and the draft's first load is in (typing before it is lost).
   */
  async compose(): Promise<void> {
    await this.press("c");
    await expect(this.page.getByRole("heading", { name: "New Message" })).toBeVisible();
    await this.settle();
    await expect(this.field("To")).toBeFocused();
  }

  /**
   * Presses `r`, `Enter` (reply all, in a conversation), or `f`, and waits
   * until the reply editor has focus and its draft is loaded. On the list,
   * `r` and `f` open the conversation first.
   */
  async reply(key: "r" | "Enter" | "f" = "r"): Promise<void> {
    await this.press(key);
    await expect(this.editor).toBeFocused();
    await this.settle();
  }

  field(label: "To" | "Cc" | "Bcc"): Locator {
    return this.page.getByRole("combobox", { name: label, exact: true });
  }

  get editor(): Locator {
    return this.page.locator(".ProseMirror");
  }

  get subject(): Locator {
    return this.page.getByPlaceholder("Subject");
  }

  /** Recipient chips in a field; each carries `data-chip` with its address. */
  chips(label: "To" | "Cc" | "Bcc"): Locator {
    return this.field(label).locator("xpath=..").locator("[data-chip]");
  }

  /** Chips whose address is not valid (shown in `danger`). */
  invalidChips(label: "To" | "Cc" | "Bcc"): Locator {
    return this.chips(label).and(this.page.locator('[aria-invalid="true"]'));
  }

  /** Suggestions open under a recipient field. */
  suggestions(label: "To" | "Cc" | "Bcc" = "To"): Locator {
    return this.page.locator(`#${label}-suggestions`).getByRole("option");
  }

  // ---------------------------------------------------------------- rail and first run

  get firstRunButton(): Locator {
    return this.page.getByRole("button", { name: "Add a Gmail account" });
  }

  /** An account's avatar in the rail. */
  account(email: string): Locator {
    return this.page
      .getByRole("navigation")
      .getByRole("button", { description: new RegExp(email.replace(/[.+]/g, "\\$&")) });
  }

  /** The quiet sync status marker on an account's avatar (SPEC Gmail errors). */
  syncMarker(email: string): Locator {
    return this.account(email).locator('[data-marker="retrying"]');
  }

  /** The Sign in again marker on an account's avatar. */
  signInMarker(email: string): Locator {
    return this.account(email).locator('[data-marker="signin"]');
  }

  /** Removes an account from Settings and closes Settings, which stays open after a removal. */
  async removeAccount(email: string): Promise<void> {
    await this.press("Meta+,");
    const settings = this.sheet("Settings");
    await settings
      .locator("div")
      .filter({ has: this.page.getByText(email, { exact: true }) })
      .filter({ has: this.page.getByRole("button", { name: "Remove account" }) })
      .last()
      .getByRole("button", { name: "Remove account" })
      .click();
    await settings.getByRole("button", { name: "Remove", exact: true }).click();
    await expect(this.account(email)).toHaveCount(0);
    await this.press("Escape");
    await expect(settings).toHaveCount(0);
  }

  /** Waits until Gmail shows `check` true for this mailbox. */
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
