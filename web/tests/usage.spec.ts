import { expect, test } from "@playwright/test";
import { ANALYSIS_ID } from "./support";
import { PR_URL, analyze, expectAccessible } from "./screens-support";

const usage = "LLM usage so far: 1.2 M input (0.9 M cached), 45 k output tokens in 7 jobs";

test("usage appears in progress, PR-context facts and Settings with its provider source", async ({
  page,
}) => {
  await page.goto(`/analyses/${ANALYSIS_ID}/progress?scenario=usage-progress`);
  await expect(page.getByText(usage)).toHaveAccessibleDescription(/Reported by the provider/);
  await page.goto(`/analyses/${ANALYSIS_ID}?scenario=usage#pr_context`);
  await expect(page.getByRole("region", { name: "Facts" }).getByText(usage)).toBeVisible();
  await page.goto("/settings?scenario=usage");
  await expect(
    page.getByText("Today: 1.2 M input (0.9 M cached), 45 k output tokens in 7 jobs"),
  ).toBeVisible();
  await expect(
    page.getByText("Last 7 days: 3.4 M input (2.9 M cached), 120 k output tokens in 23 jobs"),
  ).toBeVisible();
  await expectAccessible(page);
});

test("large PR warns before starting and Analyze remains available without a dialog", async ({ page }) => {
  await page.goto("/?scenario=large-pr");
  await analyze(page);
  await expect(page.getByRole("note")).toContainText("Large PR: 27 081 changed lines in 371 files.");
  await expect(page.getByRole("note")).toContainText("rough estimate from changed lines in recent runs");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  const field = page.getByLabel("GitHub pull request URL");
  await field.fill("https://github.com/acme/other/pull/1");
  await expect(page.getByRole("note")).toHaveCount(0);
  await field.fill(PR_URL);
  await page.getByRole("button", { name: "Analyze", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/analyses/${ANALYSIS_ID}/progress`));
});

test("without history the large-PR note makes no timing claim", async ({ page }) => {
  await page.goto("/?scenario=large-pr-no-history");
  await analyze(page);
  await expect(page.getByRole("note")).toContainText("No time estimate yet.");
  await expect(page.getByRole("note")).not.toContainText("Based on recent runs");
  await expect(page.getByRole("button", { name: "Analyze", exact: true })).toBeEnabled();
});
