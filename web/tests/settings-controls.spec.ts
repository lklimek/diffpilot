import { test, expect } from "@playwright/test";
import { ANALYSIS_ID } from "./support";
import { expectAccessible, analyze } from "./screens-support";

test("installation model saves and reports the editor; failed saves remain visible", async ({ page }) => {
  await page.goto("/settings");
  await page.getByLabel("Model", { exact: true }).selectOption("gpt-6-astra");
  await page.getByLabel("Reasoning effort").selectOption("low");
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.getByText("LLM choice saved.")).toBeVisible();
  await expect(page.getByText(/changed by alice at/)).toBeVisible();
  await expect(page.getByLabel("Model", { exact: true })).toHaveValue("gpt-6-astra");
  await page.goto("/settings?scenario=llm-save-fails");
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("The LLM choice was not saved");
});

for (const width of [1280, 2560])
  test(`compact settings and searchable picker at ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 800 });
    await page.goto("/settings?scenario=many-repos");
    await expect(page.getByText(/last success/)).toContainText("17 Nov 2026, 18:04");
    const repoTables = page.getByRole("region", { name: /^Repository / }).getByRole("table");
    await expect(repoTables).toHaveCount(0);
    const combo = page.getByRole("combobox", { name: "Repository", exact: true });
    await combo.fill("repo-25");
    await combo.press("ArrowDown");
    await combo.press("Enter");
    await expect(page).toHaveURL(/repo=acme%2Frepo-25/);
    await page.reload();
    await expect(combo).toHaveValue("acme/repo-25");
    await page.goto("/settings?scenario=many-repos");
    await expect(combo).toHaveValue("acme/repo-25");
    await combo.fill("very-long");
    await combo.press("ArrowDown");
    await combo.press("Enter");
    await page.getByText("Show the 4 built-in rules").click();
    await expect(repoTables).toBeVisible();
    for (const title of ["LLM status", "Memory", "Language of LLM texts", "GitHub App"]) {
      const extra = await page.getByRole("region", { name: title, exact: true }).evaluate((el) => {
        const last = el.lastElementChild!.getBoundingClientRect();
        const rect = el.getBoundingClientRect();
        return rect.bottom - last.bottom;
      });
      expect(extra).toBeLessThan(24);
    }
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
    await page.screenshot({ path: `test-results/settings-${width}.png`, fullPage: true });
    await expectAccessible(page);
  });

test("pause and resume retain progress; cancellation explains loss before confirmation", async ({ page }) => {
  await page.clock.install();
  await page.goto(`/analyses/${ANALYSIS_ID}/progress?scenario=estimate-recent`);
  await page.getByRole("button", { name: "Pause analysis", exact: true }).click();
  await expect(page.getByText(/Paused by alice/)).toBeVisible();
  await page.clock.fastForward(60_000);
  await expect(page.getByRole("button", { name: "Resume", exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Cancel analysis", exact: true }).click();
  await expect(page.getByRole("group", { name: "Confirm cancellation" })).toContainText(
    "Pause keeps everything.",
  );
  await page.getByRole("button", { name: "Keep analysis" }).click();
  await page.getByRole("button", { name: "Resume", exact: true }).click();
  await expect(page.getByText(/Paused by alice/)).toHaveCount(0);
});

test("Home offers Resume for a user pause", async ({ page }) => {
  await page.goto("/review?scenario=user-paused");
  await analyze(page);
  await expect(page.getByText(/Resume when you want/)).toBeVisible();
  await page.getByRole("button", { name: "Resume", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/analyses/${ANALYSIS_ID}/progress`));
});
