import { defineConfig, devices } from "@playwright/test";

// End-to-end tests: the built UI in WebKit (the engine behind Tauri's macOS
// webview) against the real Rust core and a fake Gmail. Each test gets its own
// server process and database (see e2e/fixtures.ts).
export default defineConfig({
  testDir: "./e2e",
  globalSetup: "./e2e/global-setup.ts",
  fullyParallel: true,
  // Each worker drives its own WebKit window. Several full runs at once (as
  // parallel agents did) exhausted macOS WindowServer's image buffers and
  // crashed the login session, so one run stays small.
  workers: 4,
  forbidOnly: !!process.env.CI,
  retries: 0,
  timeout: 30_000,
  expect: { timeout: 5_000 },
  reporter: [["list"]],
  use: {
    ...devices["Desktop Safari"],
    viewport: { width: 1280, height: 800 },
    // The core runs in the same zone (e2e/fixtures.ts).
    timezoneId: "America/New_York",
    // DOM snapshots only: trace screenshots capture the window continuously,
    // which is what WindowServer was serving when it ran out of buffers.
    trace: { mode: "retain-on-failure", screenshots: false },
  },
  projects: [{ name: "webkit", use: { browserName: "webkit" } }],
});
