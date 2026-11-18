import { expect, test, type Page } from "@playwright/test";

// Screenshots of every My work msw scenario at 1600×900 and 3440×1440 (light; `normal` also dark), plus `normal`
// with an item expanded and the side panel open, and the pages the review-start rows lead to. Mock data only.
// Runs only with SCREENSHOT_DIR set, e.g. SCREENSHOT_DIR=/data/artifacts/smart-review/<date>/v1.1-dashboard-screens.
const dir = process.env.SCREENSHOT_DIR;

/** 10:27 in Warsaw on the fixture day, as in the mockup: after every fixture scan (08:14, 09:24). */
const NOW = new Date("2026-11-18T09:27:00Z");
const SCENARIOS = [
  "normal",
  "refreshing",
  "provisional",
  "waiting",
  "ranking-failed",
  "empty",
  "error",
  "first-run",
  "no-time-zone",
  "no-time-zone-refused",
  "review-new",
  "review-found",
  "review-moved",
];
const NC482 = "feat(rpc): retry with backoff on timeout";

async function openDashboard(page: Page, scenario: string) {
  await page.goto(`/?dashboard=${scenario}`);
  await expect(page.getByRole("heading", { name: "My work", level: 1 })).toBeVisible();
  await expect(page.getByText(/^Loading/)).toHaveCount(0);
  await page.waitForTimeout(300);
}

/** A full-page screenshot from the top (sticky bars stay at the top). */
async function shoot(page: Page, name: string) {
  await page.evaluate(() => window.scrollTo(0, 0));
  await page.screenshot({ path: `${dir}/${name}.png`, fullPage: true });
}

/** Expands the first ranked item and opens the side panel with all its sections. */
async function expandAll(page: Page) {
  await page.locator("li[data-item] button[aria-expanded]").first().click();
  const rail = page.getByRole("button", { name: /^More/ });
  if (await rail.isVisible()) await rail.click();
  const closed = page.locator("aside details:not([open]) > summary");
  while ((await closed.count()) > 0) await closed.first().click();
  await page.waitForTimeout(300);
}

for (const [width, height] of [
  [1600, 900],
  [3440, 1440],
] as const) {
  test(`dashboard screens ${width}x${height}`, async ({ browser }) => {
    test.skip(!dir, "SCREENSHOT_DIR not set");
    test.setTimeout(240_000);
    for (const colorScheme of ["light", "dark"] as const) {
      const context = await browser.newContext({
        colorScheme,
        viewport: { width, height },
        timezoneId: "Europe/Warsaw",
      });
      // `no-time-zone` is shot before R6b stores the zone: its PUT is held for that page.
      await context.addInitScript(() => {
        if (new URLSearchParams(location.search).get("dashboard") !== "no-time-zone") return;
        const send = window.fetch.bind(window);
        window.fetch = (input, init) =>
          init?.method === "PUT" && String(input).endsWith("/settings/dashboard/time-zone")
            ? new Promise<Response>(() => {})
            : send(input, init);
      });
      const page = await context.newPage();
      await page.clock.setFixedTime(NOW);
      const scenarios = colorScheme === "light" ? SCENARIOS : ["normal"];
      for (const scenario of scenarios) {
        await openDashboard(page, scenario);
        await shoot(page, `dashboard-${scenario}-${colorScheme}-${width}`);
      }
      await openDashboard(page, "normal");
      await expandAll(page);
      await shoot(page, `dashboard-normal-expanded-${colorScheme}-${width}`);
      if (colorScheme === "light") {
        // Where the NC-482 row leads: review start at /review (new, moved; checked on arrival) or the review
        // itself (found).
        for (const [scenario, link] of [
          ["review-new", "Review"],
          ["review-moved", "Open review"],
          ["review-found", "Resume review"],
        ] as const) {
          await openDashboard(page, scenario);
          await page
            .getByRole("listitem")
            .filter({ hasText: NC482 })
            .getByRole("link", { name: link })
            .click();
          if (scenario !== "review-found")
            await expect(page.getByRole("region", { name: "Pull request check" })).toBeVisible();
          await page.waitForTimeout(800);
          await shoot(page, `review-start-${scenario.slice("review-".length)}-${colorScheme}-${width}`);
        }
      }
      await context.close();
    }
  });
}
