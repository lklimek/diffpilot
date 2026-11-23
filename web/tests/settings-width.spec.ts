import { expect, test } from "./fixtures";

// Owner 2026-11-23 (U10): at 1280 px the settings page was 1487 px wide. The screen-reader text of the wide tables
// (absolutely placed) escaped their scroll wrappers; the tables scroll in their cards and the page stays put.
test("settings at 1280 px: wide tables scroll in their cards, the page does not widen", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto("/settings");
  const dashboard = page.getByRole("region", { name: "Dashboard", exact: true });
  const repositories = dashboard.getByRole("table").first();
  await expect(repositories).toBeVisible();
  await expect(dashboard.getByRole("table", { name: "Slack names" })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(1280);
  // Nothing is hidden: the table's last column comes into its wrapper's view by scrolling the wrapper.
  const remove = repositories.getByRole("button", { name: /^Remove / }).first();
  await remove.scrollIntoViewIfNeeded();
  const inside = await remove.evaluate((button) => {
    const wrapper = button.closest("table")!.parentElement!;
    wrapper.scrollTo({ left: wrapper.scrollWidth });
    const outer = wrapper.getBoundingClientRect();
    const own = button.getBoundingClientRect();
    return own.left >= outer.left - 1 && own.right <= outer.right + 1 && own.width > 0;
  });
  expect(inside).toBe(true);
});
