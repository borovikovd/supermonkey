// The app as a user sees it: page helpers shared by every spec. Prefer
// role- and label-based locators; add helpers here rather than repeating
// selectors across specs.

import { expect, type Locator, type Page } from "@playwright/test";

import type { Control, GmailState } from "./control.ts";

export class App {
  readonly page: Page;
  readonly gmail: Control;

  constructor(page: Page, gmail: Control) {
    this.page = page;
    this.gmail = gmail;
  }

  /**
   * Starts the engine and loads the UI. Seed accounts and mail first.
   * The browser clock is installed so `advance` can move time on both sides.
   */
  async open(): Promise<void> {
    await this.page.clock.install();
    await this.gmail.start();
    await this.page.goto(this.gmail.url);
    await expect(this.page.getByRole("navigation")).toBeVisible();
  }

  /** Moves app time forward in the core and the browser together. */
  async advance(ms: number): Promise<void> {
    await this.gmail.advanceServerClock(ms);
    await this.page.clock.fastForward(ms);
  }

  /** Presses keys in order, e.g. `press("g", "i")` or `press("Meta+k")`. */
  async press(...keys: string[]): Promise<void> {
    // In order: each key must land before the next.
    await keys.reduce((prev, k) => prev.then(() => this.page.keyboard.press(k)), Promise.resolve());
  }

  async type(text: string): Promise<void> {
    await this.page.keyboard.type(text);
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

  get highlighted(): Locator {
    return this.list.locator('[role="option"][aria-selected="true"]');
  }

  /** Subjects of the visible rows, top to bottom. */
  async subjects(): Promise<string[]> {
    return this.rows.evaluateAll((els) =>
      els.map((el) => el.querySelector(".truncate.text-text")?.firstChild?.textContent ?? ""),
    );
  }

  /** The split names in the Inbox title, in order. */
  splitTitles(): Locator {
    return this.page.locator("header button, div > button").filter({ hasText: /\S/ });
  }

  // ---------------------------------------------------------------- conversation

  get conversation(): Locator {
    return this.page.getByRole("main");
  }

  get conversationTitle(): Locator {
    return this.conversation.getByRole("heading", { level: 1 });
  }

  /** Message headers in the open conversation (expanded and collapsed). */
  get messages(): Locator {
    return this.conversation.locator("[aria-expanded]");
  }

  /** A message body frame by its position in the conversation. */
  messageBody(nth: number) {
    return this.conversation.locator("iframe[data-frame]").nth(nth).contentFrame();
  }

  // ---------------------------------------------------------------- overlays

  get palette(): Locator {
    return this.page.getByRole("dialog").filter({ has: this.page.getByRole("combobox") });
  }

  sheet(title: string | RegExp): Locator {
    return this.page.getByRole("dialog", { name: title });
  }

  get toasts(): Locator {
    return this.page.locator("[data-sonner-toast]");
  }

  toast(text: string | RegExp): Locator {
    return this.toasts.filter({ hasText: text });
  }

  // ---------------------------------------------------------------- composer

  field(label: "To" | "Cc" | "Bcc"): Locator {
    return this.page.getByRole("combobox", { name: label });
  }

  get editor(): Locator {
    return this.page.locator(".ProseMirror");
  }

  /** Recipient chips in a field. */
  chips(label: "To" | "Cc" | "Bcc"): Locator {
    return this.field(label).locator("xpath=..").locator("[data-chip]");
  }

  // ---------------------------------------------------------------- rail

  account(email: string): Locator {
    return this.page
      .getByRole("navigation")
      .getByRole("button", { description: new RegExp(email) });
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
