import { test as base } from "@playwright/test";

/**
 * The browser's clock in every msw spec: shortly after the fixtures' scan (2026-11-18 08:14 UTC), so times render as
 * on the fixtures' day whatever the wall clock says. Time flows from there; specs may still pause or jump it.
 */
export const PINNED_NOW = new Date("2026-11-18T09:27:00Z");

/** Playwright's `test` with the pinned clock installed before the page loads. */
export const test = base.extend({
  page: async ({ page }, use) => {
    await page.clock.install({ time: PINNED_NOW });
    await use(page);
  },
});

export { expect } from "@playwright/test";
