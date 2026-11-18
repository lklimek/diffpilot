import { expect, test, type Page } from "@playwright/test";
import { expectAccessible, expectKeyboardTooltips } from "./screens-support";
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
    const show = main.getByRole("button", { name: "Show 2 hidden comment/import lines" });
    await expect(show).toBeVisible();
    expect((await show.boundingBox())!.height).toBeGreaterThanOrEqual(24);
    await page.screenshot({ path: info.outputPath(`remark-layout-${colorScheme}.png`) });
    await show.click();
    await expect(line(page, "head", 93).locator("[data-mark=changed]")).toContainText("+");
    await main.getByRole("button", { name: "Hide 2 comment/import lines" }).click();
    await expect(line(page, "head", 93)).toHaveCount(0);

    // The changed sign is read with its line (no tooltip trigger); the remark marker explains itself.
    await expect(line(page, "head", 90).locator("[data-mark=changed]")).toContainText("added line");
    await line(page, "head", 90).getByRole("button", { name: "LLM remark at line 90" }).hover();
    await expect(page.getByRole("dialog", { name: "LLM remarks at line 90" })).toContainText(
      "Was: checks the wallet",
    );
    await expectAccessible(page);
    // Screen-wide: every tooltip trigger on the Tour with changed lines takes keyboard focus (pointer away, so no
    // hover popover is open).
    await page.mouse.move(0, 0);
    await expect(page.getByRole("dialog", { name: "LLM remarks at line 90" })).toBeHidden();
    await expectKeyboardTooltips(page);
  });
}
