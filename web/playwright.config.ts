import { defineConfig, devices } from "@playwright/test";

const PORT = 5174;

// Runs against the Vite dev server with msw (`npm run dev:mock`); one run at a time, 2 workers (host limit).
export default defineConfig({
  testDir: "tests",
  workers: 2,
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  reporter: [["list"]],
  use: {
    baseURL: `http://localhost:${PORT}`,
    ...devices["Desktop Chrome"],
    viewport: { width: 1440, height: 900 },
    trace: "retain-on-failure",
  },
  webServer: {
    command: `npm run dev:mock -- --port ${PORT} --strictPort`,
    url: `http://localhost:${PORT}`,
    reuseExistingServer: false,
    timeout: 60_000,
  },
});
