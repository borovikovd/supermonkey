// Security (SPEC Reading and Security): hostile HTML, markup in
// metadata, links, and no Gmail credentials in the webview.
import type { Dialog } from "@playwright/test";

import type { App } from "./app.ts";
import { expect, test } from "./fixtures.ts";
import * as seeds from "./seeds.ts";

/** The type of `__pwned` on the app page and in every message frame ("undefined" when unset). */
function pwned(app: App): Promise<string[]> {
  return app.page.evaluate(() =>
    [window, ...Array.from(document.querySelectorAll("iframe")).map((f) => f.contentWindow)].map(
      (w) => (w ? typeof Reflect.get(w, "__pwned") : "no window"),
    ),
  );
}

/**
 * Everything the webview keeps: localStorage, sessionStorage, IndexedDB, the
 * `window.__ub` stores as JSON, and the DOM. A string, because `__ub` is the
 * app's own hook (src/main.tsx).
 */
const WEBVIEW_STATE = `(async () => {
  const dump = (s) => Array.from({ length: s.length }, (_, i) => s.key(i) + "=" + s.getItem(s.key(i))).join("\\n");
  const request = (r) => new Promise((resolve, reject) => {
    r.onsuccess = () => resolve(r.result);
    r.onerror = () => reject(r.error);
  });
  const dbs = await indexedDB.databases();
  const idb = await Promise.all(dbs.map(async (info) => {
    const db = await request(indexedDB.open(info.name));
    const stores = await Promise.all(Array.from(db.objectStoreNames).map((n) =>
      request(db.transaction(n).objectStore(n).getAll()).then((all) => JSON.stringify(all))));
    db.close();
    return stores.join("\\n");
  }));
  const json = (v) => JSON.stringify(v, (_k, x) => (x instanceof Map ? Object.fromEntries(x) : x));
  const stores = [window.__ub.useChrome.getState(), window.__ub.useMirror.getState()].map(json);
  return [dump(localStorage), dump(sessionStorage), ...idb, ...stores, document.documentElement.outerHTML].join("\\n");
})()`;

const HOSTILE_SUBJECT = "<img src=x onerror=top.__pwned=1>";
const HOSTILE_NAME = "<b>Boss</b>";

test.describe("security", () => {
  test("SEC-01 hostile HTML cannot run script or navigate", async ({ app, gmail }) => {
    const dialogs: Dialog[] = [];
    app.page.on("dialog", (d) => {
      dialogs.push(d);
      void d.dismiss();
    });
    await seeds.security(gmail, app);
    const url = app.page.url();
    await app.row("Hostile").click();
    const body = app.messageBody(0);
    await expect(body.getByText("Hostile content.")).toBeVisible();
    await app.press("Shift+O");
    await expect(body.getByText("Hostile content.")).toBeVisible();

    // Click what is left of the javascript: link and the form's submit button.
    await body.getByText("Run me").click();
    await body.getByText("Submit").click();
    // Tab to each link and open it.
    const links = await body.locator("a[href]").count();
    await app.press(...Array.from({ length: links }, () => ["Tab", "Enter"]).flat());
    await app.advance(1_000);
    await app.settle();

    // Script and dialogs.
    expect(await pwned(app)).toEqual(["undefined", "undefined"]);
    expect(dialogs).toEqual([]);

    // Navigation.
    expect(app.page.url()).toBe(url);
    const { openedUrls } = await gmail.platform();
    expect(openedUrls.filter((u) => /^javascript:|evil\.test/i.test(u))).toEqual([]);
    expect(app.popups.filter((u) => /^javascript:|evil\.test/i.test(u))).toEqual([]);
    expect((await gmail.hits()).filter((h) => h.path.startsWith("/sink/evil.test"))).toEqual([]);

    // The frame.
    const frame = app.conversation.locator("iframe[data-frame]");
    await expect(frame).toHaveAttribute("sandbox", "allow-same-origin allow-scripts");
    const dom = await frame.evaluate((f) => {
      const doc = f instanceof HTMLIFrameElement ? f.contentDocument : null;
      if (!doc) return null;
      const head = Array.from(doc.head.children);
      // The policy comes before anything but the charset.
      const first = head.find((el) => !el.matches("meta[charset]"));
      const all = Array.from(doc.querySelectorAll("*"));
      return {
        firstInHead: first?.outerHTML ?? "",
        csp: first?.getAttribute("content") ?? "",
        scripts: doc.querySelectorAll("script").length,
        handlers: all.flatMap((el) =>
          el.getAttributeNames().filter((n) => n.toLowerCase().startsWith("on")),
        ),
        jsUrls: all.flatMap((el) =>
          el.getAttributeNames().filter((n) => /javascript:/i.test(el.getAttribute(n) ?? "")),
        ),
        forbidden: Array.from(doc.querySelectorAll("form, iframe, base")).map((el) => el.tagName),
        refresh: all.filter(
          (el) =>
            el.tagName === "META" && el.getAttribute("http-equiv")?.toLowerCase() === "refresh",
        ).length,
      };
    });
    expect(dom).not.toBeNull();
    expect(dom?.firstInHead).toMatch(/^<meta http-equiv="Content-Security-Policy"/i);
    expect(dom?.csp).toContain("script-src 'none'");
    expect(dom?.scripts).toBe(0);
    expect(dom?.handlers).toEqual([]);
    expect(dom?.jsUrls).toEqual([]);
    expect(dom?.forbidden).toEqual([]);
    expect(dom?.refresh).toBe(0);
  });

  test("SEC-02 mail text never becomes markup outside the frame", async ({ app, gmail }) => {
    await seeds.security(gmail, app);
    const row = app.row(HOSTILE_SUBJECT);

    // The list row.
    await expect(row).toHaveCount(1);
    await expect(row).toContainText(HOSTILE_NAME);
    await expect(row.locator("img, b, script")).toHaveCount(0);

    // The conversation.
    await row.click();
    await expect(app.conversationTitle).toHaveText(HOSTILE_SUBJECT);
    await expect(app.conversation).toContainText(HOSTILE_NAME);
    await expect(app.conversation.locator("img, b, script")).toHaveCount(0);

    // Pin its sender with Move to Important.
    await app.press("Meta+k");
    await app.type("Move to Important");
    await app.press("Enter");
    const pin = app.sheet("Move to Important");
    await expect(pin.locator("[cmdk-item]").first()).toContainText("boss@meta.test");
    await expect(pin.locator("img, b, script")).toHaveCount(0);
    await app.press("Enter");
    await expect(pin).toHaveCount(0);

    // Reply and send it, so its subject shows in the send toast.
    await app.reply("r");
    await app.type("Noted.");
    await app.settle();
    await app.press("Meta+Enter");
    const toast = app.toast(HOSTILE_SUBJECT);
    await expect(toast).toHaveCount(1);
    await expect(app.toasts.locator("img, b, script")).toHaveCount(0);

    // Its name in autocomplete.
    await app.press("Escape");
    await app.compose();
    await app.type("Bo");
    await expect(app.suggestions("To").filter({ hasText: HOSTILE_NAME })).toHaveCount(1);
    // The list bolds the matched part itself; an injected <b> would hold "Boss".
    const list = app.page.locator("#To-suggestions");
    await expect(list.locator("img, script")).toHaveCount(0);
    await expect(list.locator("b").filter({ hasText: /^Boss$/ })).toHaveCount(0);

    await app.settle();
    expect((await pwned(app)).filter((x) => x !== "undefined")).toEqual([]);
  });

  test("SEC-03 links never navigate the app window", async ({ app, gmail }) => {
    await seeds.standard(gmail, app);
    const url = app.page.url();
    // The message frame must not follow a link either (SPEC Reading: clicks are caught).
    const framesLoaded: string[] = [];
    app.page.on("framenavigated", (f) => {
      if (f !== app.page.mainFrame() && !f.url().startsWith("about:")) framesLoaded.push(f.url());
    });
    await app.row("Links").click();
    const body = app.messageBody(0);
    const opened = async () => (await gmail.platform()).openedUrls;

    await body.getByRole("link", { name: "Link A" }).click();
    await expect.poll(opened).toEqual(["https://example.org/a"]);
    expect(app.page.url()).toBe(url);

    await body.getByRole("link", { name: "Mail X" }).click();
    await expect.poll(opened).toEqual(["https://example.org/a", "mailto:x@example.org"]);

    await body.getByRole("link", { name: "Top link" }).click();
    await expect
      .poll(opened)
      .toEqual(["https://example.org/a", "mailto:x@example.org", "https://example.org/top"]);
    await app.settle();
    expect(app.page.url()).toBe(url);
    await expect(app.conversationTitle).toHaveText("Links");

    await app.press("Tab", "Enter");
    await expect
      .poll(opened)
      .toEqual([
        "https://example.org/a",
        "mailto:x@example.org",
        "https://example.org/top",
        "https://example.org/a",
      ]);
    await app.settle();
    expect(app.page.url()).toBe(url);
    expect(app.popups).toEqual([]);
    expect(framesLoaded).toEqual([]);
    await expect(body.getByRole("link", { name: "Link B" })).toBeVisible();
  });

  test("SEC-07 links in a reply's quoted text open in the browser, never in the composer", async ({
    app,
    gmail,
  }) => {
    await seeds.standard(gmail, app);
    await gmail.deliver({
      account: seeds.A,
      from: "Mallory Mal <mallory@evil.test>",
      subject: "Quoted links",
      html:
        '<p><a href="http://evil.test/quoted">Tracked link</a></p>' +
        '<p><a href="mailto:x@example.org">Mail X</a></p>' +
        '<p><a href="ftp://evil.test/file">Other scheme</a></p>',
    });
    const framesLoaded: string[] = [];
    app.page.on("framenavigated", (f) => {
      if (f !== app.page.mainFrame() && !f.url().startsWith("about:")) framesLoaded.push(f.url());
    });
    await app.row("Quoted links").click();
    await expect(app.conversationTitle).toHaveText("Quoted links");
    await app.reply("r");
    await app.page.getByTitle("Show quoted text").click();
    const quote = app.page.locator('iframe[title="quoted"]').contentFrame();
    const opened = async () => (await gmail.platform()).openedUrls;

    await quote.getByRole("link", { name: "Tracked link" }).click();
    await expect.poll(opened).toEqual(["http://evil.test/quoted"]);
    await quote.getByRole("link", { name: "Mail X" }).click();
    await expect.poll(opened).toEqual(["http://evil.test/quoted", "mailto:x@example.org"]);
    await quote.getByRole("link", { name: "Other scheme" }).click();
    await app.settle();

    // Nothing else opened, and the quote is still the quote, in the composer.
    expect(await opened()).toEqual(["http://evil.test/quoted", "mailto:x@example.org"]);
    await expect(quote.getByRole("link", { name: "Tracked link" })).toBeVisible();
    await expect(app.editor).toBeVisible();
    expect(framesLoaded).toEqual([]);
    expect(app.popups).toEqual([]);
    expect((await gmail.hits()).filter((h) => h.path.startsWith("/sink/evil.test"))).toEqual([]);
  });

  test("SEC-04 the webview holds no Gmail credentials", async ({ app, gmail }) => {
    await seeds.standard(gmail, app);
    const tokens = (await gmail.tokens()).map((k) => k.token);
    expect(tokens.length).toBeGreaterThan(0);

    const stored = await app.page.evaluate<string>(WEBVIEW_STATE);
    expect(stored).toContain("Q4 plan");
    expect(tokens.filter((t) => stored.includes(t))).toEqual([]);
    expect(await Promise.all(tokens.map((t) => gmail.dbContains(t)))).toEqual(
      tokens.map(() => false),
    );

    // Every Gmail request came from the core, not the browser.
    const log = await gmail.requests();
    expect(log.length).toBeGreaterThan(0);
    expect(log.filter((r) => r.origin !== null)).toEqual([]);
    expect(
      log.filter((r) => /Mozilla|AppleWebKit|Safari/.test(r.userAgent ?? "")).map((r) => r.path),
    ).toEqual([]);
  });
});

test("SEC-06 the UI runs under the release app's content security policy", async ({
  app,
  gmail,
}) => {
  await seeds.standard(gmail, app);
  await expect(app.rows.first()).toBeVisible();
  // A script added inline does not run, as in the release app; the app's
  // own scripts and its channel to the core work.
  const ran = await app.page.evaluate(() => {
    const script = document.createElement("script");
    script.textContent = "document.body.dataset.inlineRan = 'yes'";
    document.body.append(script);
    return document.body.dataset.inlineRan ?? "no";
  });
  expect(ran).toBe("no");
  await app.press("j");
  await app.press("Enter");
  await expect(app.conversationTitle).toBeVisible();
});
