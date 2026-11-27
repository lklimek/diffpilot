import { expect, test } from "./fixtures";

// TODO(v1.4 phase A): Run on the coordinator’s browser-enabled host.
for (const width of [390, 1280]) {
  test(`repositories scroll inside the page at ${width} px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 800 });
    await page.goto("/settings/repositories");
    const table = page.getByRole("table", { name: "Repositories", exact: true });
    await expect(table).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
    const action = table.getByRole("button", { name: /^Show analysis rules for acme\/node-client$/ });
    await action.scrollIntoViewIfNeeded();
    await action.press("Enter");
    await expect(
      table.getByRole("button", { name: "Hide analysis rules for acme/node-client" }),
    ).toHaveAttribute("aria-expanded", "true");
    await expect(page.getByText("Project profile source")).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
  });
}
