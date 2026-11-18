import { expect, test } from "@playwright/test";
import { ANALYSIS_ID } from "./support";

for (const width of [1280, 2560]) {
  test(`compact review at ${width}px keeps evidence and navigation usable`, async ({ page }, info) => {
    await page.setViewportSize({ width, height: width === 1280 ? 800 : 1080 });
    await page.goto(`/analyses/${ANALYSIS_ID}?scenario=show-less#f1.2`);
    const inspector = page.getByRole("complementary", { name: "Inspector" });
    await expect(page.getByRole("main").getByRole("heading", { name: /^2 / })).toBeVisible();
    await expect(page.getByText("Assessment (LLM)")).toHaveCount(0);
    await expect(inspector.getByRole("button", { name: /Call hierarchy/ })).toHaveCount(0);
    const hide = page.getByRole("checkbox", { name: "Hide comment, import and whitespace changes" });
    await expect(hide).toBeChecked();
    await expect(
      page.getByText("2 comment, import or whitespace lines hidden — in the classic diff"),
    ).toBeVisible();
    const mainColumn = page.locator("[data-column=main]");
    const decision = await page
      .getByRole("main")
      .getByText("Counts the attempt; rebuilds the request on every turn.")
      .boundingBox();
    const oldCode = await page.getByRole("button", { name: "Show old code", exact: true }).boundingBox();
    expect(Math.abs(decision!.y - oldCode!.y)).toBeLessThanOrEqual(4);
    expect(await mainColumn.evaluate((el) => el.scrollWidth <= el.clientWidth)).toBe(true);
    expect(
      await page.getByRole("main").evaluate((el) => el.scrollWidth <= el.clientWidth && el.scrollLeft === 0),
    ).toBe(true);
    await page.screenshot({ path: info.outputPath("step-code.png") });
    await hide.uncheck();
    await expect(
      page.getByText("2 comment, import or whitespace lines hidden — in the classic diff"),
    ).toHaveCount(0);
    await page.getByRole("navigation").getByRole("button", { name: "Step 5: Retry allowed?" }).click();
    const evidence = inspector.getByText(/Evidence ·/).first();
    await evidence.click();
    await expect(inspector.getByRole("button", { name: /Show .*:/ }).first()).toBeVisible();
    await inspector
      .getByRole("button", { name: /Show .*:/ })
      .first()
      .scrollIntoViewIfNeeded();
    await inspector.screenshot({ path: info.outputPath("inspector-evidence.png") });
    await page.getByRole("button", { name: /Show flow diagram/ }).click();
    await expect(page.locator("[data-step='f1.5']")).toHaveAttribute("aria-description", /attempt/);
    await page
      .getByRole("region", { name: "Flow diagram", exact: true })
      .screenshot({ path: info.outputPath("diagram.png") });
    await page.getByRole("navigation").screenshot({ path: info.outputPath("navigation.png") });
    await page
      .getByRole("navigation")
      .getByRole("button", { name: /^Flow 2:/ })
      .click();
    await page
      .getByRole("navigation")
      .getByRole("button", { name: /^Step 1:/ })
      .click();
    await expect(page.getByText("Changed: 1 line replaced")).toBeVisible();
    const article = await page.getByRole("main").locator("article").first().boundingBox();
    const code = await page.getByRole("main").locator("[data-code-slot]").first().boundingBox();
    expect(code!.width).toBeGreaterThanOrEqual(article!.width - 26);
    await page.screenshot({ path: info.outputPath("replacement.png") });
    await page.getByRole("button", { name: "PR context", exact: true }).click();
    await page.screenshot({ path: info.outputPath("pr-context.png") });
    await page
      .getByRole("navigation")
      .getByRole("button", { name: /^Tests/ })
      .click();
    await expect(
      page.getByRole("main").getByText("Tests: 4 files, +30 −12 lines — not analysed by the LLM"),
    ).toBeVisible();
    await page.getByRole("button", { name: "Show classic diff", exact: true }).first().click();
    await expect(page.getByRole("main").getByRole("region", { name: /^Classic diff of/ })).toBeVisible();
    await page.screenshot({ path: info.outputPath("tests-diff.png") });
    await page.reload();
    await expect(hide).not.toBeChecked();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  });
}
