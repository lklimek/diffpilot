import { defineConfig, devices } from "@playwright/test";

// Full stack, no mocks in the browser: every spec file starts its own stack (support/stack.ts) from the binaries
// the global setup resolves. One Playwright run at a time on this host; at most 2 workers. Against one
// already-running stack (SR_E2E_BASE_URL) the files share it, so they run one at a time.
export default defineConfig({
  testDir: "tests",
  globalSetup: "./global-setup.ts",
  workers: process.env.SR_E2E_BASE_URL ? 1 : 2,
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  timeout: 180_000,
  reporter: [["list"]],
  outputDir: process.env.SR_E2E_OUTPUT_DIR ?? "test-results",
  use: {
    ...devices["Desktop Chrome"],
    viewport: { width: 1440, height: 900 },
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },
});
