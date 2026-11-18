import { expect, test, type Page } from "@playwright/test";
import { expectAccessible } from "./screens-support";
import { ANALYSIS_ID } from "./support";

// Owner screenshot: one LLM remark over a replaced line, unchanged lines and added comment lines that fold.
const line = (page: Page, side: "head" | "base", n: number) =>
  page.getByRole("main").locator(`[data-line="${n}"][data-side="${side}"]`);
const background = (page: Page, side: "head" | "base", n: number) =>
  line(page, side, n).evaluate((el) => getComputedStyle(el).backgroundColor);

for (const colorScheme of ["light", "dark"] as const) {
  test(`one remark, one label; changes in the classic diff's colours; folds in place (${colorScheme})`, async ({
    page,
  }, info) => {
    await page.emulateMedia({ colorScheme });
    await page.goto(`/analyses/${ANALYSIS_ID}?scenario=remark-layout#f1.2`);
    const main = page.getByRole("main");
    await expect(main.getByText("● LOW · LLM remark: behaviour")).toHaveCount(1);
    await expect(main.locator("[data-mark=remark]")).toHaveCount(1);
    // The label is above the replaced block: removed 99 and its replacement 90 stay together.
    const label = await main.getByText("● LOW · LLM remark: behaviour").boundingBox();
    const changed = await main.getByText("Changed: 1 line replaced").boundingBox();
    expect(label!.y).toBeLessThan(changed!.y);

    // Unchanged lines inside the remark look like unchanged lines outside it; changes use the classic diff's fill.
    expect(await background(page, "head", 91)).toBe(await background(page, "head", 88));
    const added = await page
      .locator("[data-diff-line=added]")
      .first()
      .evaluate((el) => getComputedStyle(el).backgroundColor);
    expect(await background(page, "head", 90)).toBe(added);
    expect(await background(page, "head", 95)).toBe(added);
    const removed = await page
      .locator("[data-diff-line=removed]")
      .first()
      .evaluate((el) => getComputedStyle(el).backgroundColor);
    expect(await background(page, "base", 99)).toBe(removed);

    // Hidden comment lines fold in place, inside the remark's bar, and open there.
    await expect(line(page, "head", 93)).toHaveCount(0);
    const show = main.getByRole("button", { name: "Show 2 hidden comment lines" });
    await expect(show).toBeVisible();
    expect((await show.boundingBox())!.height).toBeGreaterThanOrEqual(24);
    await page.screenshot({ path: info.outputPath(`remark-layout-${colorScheme}.png`) });
    await show.click();
    await expect(line(page, "head", 93).locator("[data-mark=changed]")).toContainText("+");
    await main.getByRole("button", { name: "Hide 2 comment lines" }).click();
    await expect(line(page, "head", 93)).toHaveCount(0);

    // The changed sign and the remark marker sit apart, each explained.
    await line(page, "head", 90).locator("[data-mark=changed]").hover();
    await expect(page.getByRole("tooltip").filter({ hasText: "Added line (from the diff)." })).toBeVisible();
    await line(page, "head", 90).getByRole("button", { name: "LLM remark at line 90" }).hover();
    await expect(page.getByRole("dialog", { name: "LLM remarks at line 90" })).toContainText(
      "Was: checks the wallet",
    );
    await expectAccessible(page);
  });
}
