// Playwright fixtures: every test gets its own E2E server (fresh fake Gmail
// and database) and an `App` page object bound to it.

import { spawn, type ChildProcess } from "node:child_process";
import path from "node:path";
import { createInterface } from "node:readline";

import { test as base, expect } from "@playwright/test";

import { App } from "./app.ts";
import { Control } from "./control.ts";

const SERVER = path.resolve(import.meta.dirname, "../src-tauri/target/debug/e2e-server");

async function startServer(): Promise<{ proc: ChildProcess; url: string; log: string[] }> {
  const proc = spawn(SERVER, ["--port", "0"], { stdio: ["ignore", "pipe", "pipe"] });
  const log: string[] = [];
  proc.stderr?.on("data", (b: Buffer) => log.push(b.toString()));
  const lines = createInterface({ input: proc.stdout });
  const url = await new Promise<string>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("e2e-server did not start")), 10_000);
    proc.once("exit", (code) => reject(new Error(`e2e-server exited (${code}): ${log.join("")}`)));
    lines.on("line", (line) => {
      const m = /^READY (\S+)$/.exec(line);
      if (m?.[1]) {
        clearTimeout(timer);
        resolve(m[1]);
      }
    });
  });
  return { proc, url, log };
}

type Fixtures = {
  /** The control API of this test's server. Seed with it before `app.open()`. */
  gmail: Control;
  app: App;
};

export const test = base.extend<Fixtures>({
  gmail: async ({}, use, testInfo) => {
    const { proc, url, log } = await startServer();
    await use(new Control(url));
    proc.kill();
    if (testInfo.status !== testInfo.expectedStatus) {
      await testInfo.attach("e2e-server.log", { body: log.join(""), contentType: "text/plain" });
    }
  },
  app: async ({ page, gmail }, use) => {
    await use(new App(page, gmail));
  },
});

export { expect };
