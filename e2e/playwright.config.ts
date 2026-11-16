import { defineConfig, devices } from "@playwright/test";

// Full stack (deploy/dev/stack.sh, started by the global setup): no mocks in the browser.
// One Playwright run at a time on this host; at most 2 workers.
export default defineConfig({
  testDir: "tests",
  globalSetup: "./stack.ts",
  workers: 2,
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  timeout: 180_000,
  reporter: [["list"]],
  use: {
    baseURL: process.env.SR_E2E_BASE_URL,
    ...devices["Desktop Chrome"],
    viewport: { width: 1440, height: 900 },
    trace: "retain-on-failure",
  },
});
