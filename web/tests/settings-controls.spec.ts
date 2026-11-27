import { expect, test } from "./fixtures";
import { ANALYSIS_ID } from "./support";
import { expectAccessible, analyze } from "./screens-support";

test("installation model saves and reports the editor; failed saves remain visible", async ({ page }) => {
  await page.goto("/settings/llm");
  await page.getByLabel("Model", { exact: true }).selectOption("gpt-6-astra");
  await page.getByLabel("Reasoning effort").selectOption("low");
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.getByText("Saved", { exact: true })).toBeVisible();
  await expect(page.getByText(/changed by alice at/)).toBeVisible();
  await expect(page.getByLabel("Model", { exact: true })).toHaveValue("gpt-6-astra");
  await page.goto("/settings/llm?scenario=llm-save-fails");
  await page.getByLabel("Ask effort").selectOption("low");
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("Could not save");
});

for (const width of [390, 1280, 2560]) {
  test(`settings layout at ${width} px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 800 });
    await page.goto("/settings/llm");
    const model = page.getByRole("region", { name: "Model and effort" });
    const control = model.getByLabel("Model", { exact: true });
    const help = model.getByText("The model used by new jobs across this installation.");
    await expect(control).toBeVisible();
    const [c, h] = await Promise.all([control.boundingBox(), help.boundingBox()]);
    expect(c && h).toBeTruthy();
    if (width >= 2560) expect(h!.x).toBeGreaterThan(c!.x + c!.width);
    else expect(h!.y).toBeGreaterThan(c!.y);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
    await expectAccessible(page);
    await page.emulateMedia({ colorScheme: "dark" });
    await expectAccessible(page);
  });
}

test("pause and resume retain progress; cancellation explains loss before confirmation", async ({ page }) => {
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

test("The PR panel offers Resume for a user pause", async ({ page }) => {
  await page.goto("/prs?scenario=user-paused");
  await analyze(page);
  await expect(page.getByText(/Resume when you want/)).toBeVisible();
  await page.getByRole("button", { name: "Resume", exact: true }).click();
  await expect(page).toHaveURL(/\/pr\/acme\/node-client\/482(?:\?|$)/);
});
