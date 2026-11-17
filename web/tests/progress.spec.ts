import { expect, test, type Page } from "@playwright/test";
import { expectAccessible, expectTooltips } from "./screens-support";
import { ANALYSIS_ID, expectTrustWording } from "./support";

const open = (page: Page, scenario: string) =>
  page.goto(`/analyses/${ANALYSIS_ID}/progress?scenario=${scenario}`);
const bar = (page: Page) => page.getByRole("progressbar", { name: "Analysis progress" });
const openReview = (page: Page) => page.getByRole("link", { name: "Open review" });

test("planning: indeterminate bar, Planning flows…, Cancel analysis, no Open review", async ({ page }) => {
  await open(page, "planning");
  await expect(page.getByText("Planning flows…")).toBeVisible();
  await expect(bar(page)).not.toHaveAttribute("aria-valuenow", /.*/);
  await expect(page.getByRole("button", { name: "Cancel analysis" })).toBeVisible();
  await expect(openReview(page)).toHaveCount(0);
  await expect(page.getByRole("banner")).toContainText("#482");
  await expectTooltips(page);
  await expectTrustWording(page);
  await expectAccessible(page);
});

test("running: k of n, no Open review", async ({ page }) => {
  await open(page, "running");
  await expect(page.getByText("2 of 5 flows analysed")).toBeVisible();
  await expect(bar(page)).toHaveAttribute("aria-valuenow", "2");
  await expect(openReview(page)).toHaveCount(0);
  await expectTrustWording(page);
});

test("paused: usage limit with resume time", async ({ page }) => {
  await open(page, "paused");
  await expect(
    page.getByText(/^Paused, 2 of 5 flows analysed: Codex usage limit reached, resumes at .*\d\d:\d\d$/),
  ).toBeVisible();
  await expect(bar(page)).toHaveAttribute("aria-valuenow", "2");
  await expect(openReview(page)).toHaveCount(0);
  await expectTrustWording(page);

  await open(page, "paused-unknown");
  await expect(
    page.getByText("Paused: Codex usage limit reached; the resume time is not known"),
  ).toBeVisible();
});

test("failed in a flow: names it; Retry runs the analysis to finished", async ({ page }) => {
  await open(page, "failed");
  await expect(page.getByText("Analysis of flow “Classify RPC errors” failed")).toBeVisible();
  await expect(
    page.getByText("The LLM answer did not match the expected format after 2 repairs."),
  ).toBeVisible();
  await expect(openReview(page)).toHaveCount(0);
  await expectTooltips(page);
  await expectTrustWording(page);
  await expectAccessible(page);
  await page.getByRole("button", { name: "Retry" }).click();
  await expect(page.getByText("5 of 5 flows analysed")).toBeVisible({ timeout: 10_000 });
  await expect(openReview(page)).toBeVisible();
});

test("failed before any flow: plain description and Retry", async ({ page }) => {
  await open(page, "failed-early");
  await expect(page.getByText("The analysis failed before any flow was planned")).toBeVisible();
  await expect(page.getByText(/could not be fetched from GitHub/)).toBeVisible();
  await expect(page.getByRole("button", { name: "Retry" })).toBeVisible();
  await expectTrustWording(page);
});

test("finished: n of n and Open review", async ({ page }) => {
  await open(page, "finished");
  await expect(page.getByText("5 of 5 flows analysed")).toBeVisible();
  await expect(page.getByRole("button", { name: "Cancel analysis" })).toHaveCount(0);
  await expect(openReview(page)).toHaveAttribute("href", `/analyses/${ANALYSIS_ID}`);
  await expectTooltips(page);
  await expectAccessible(page);
});

test("unknown analysis: plain error and a way back", async ({ page }) => {
  await page.goto("/analyses/00000000-0000-0000-0000-000000000000/progress");
  await expect(page.getByRole("alert")).toContainText("This analysis does not exist.");
  await expect(page.getByRole("link", { name: "Back to Home" })).toHaveAttribute("href", "/");
});
