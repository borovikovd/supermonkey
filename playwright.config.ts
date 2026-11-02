import { defineConfig, devices } from "@playwright/test";

// End-to-end tests: the built UI in WebKit (the engine behind Tauri's macOS
// webview) against the real Rust core and a fake Gmail. Each test gets its own
// server process and database (see e2e/fixtures.ts).
export default defineConfig({
  testDir: "./e2e",
  globalSetup: "./e2e/global-setup.ts",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: 0,
  timeout: 30_000,
  expect: { timeout: 5_000 },
  reporter: [["list"]],
  use: {
    ...devices["Desktop Safari"],
    viewport: { width: 1280, height: 800 },
    trace: "retain-on-failure",
  },
  projects: [{ name: "webkit", use: { browserName: "webkit" } }],
});
