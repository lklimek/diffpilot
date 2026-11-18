import { expect, test } from "@playwright/test";
import { ANALYSIS_ID } from "./support";
import { PR_URL, analyze, expectAccessible, startAnalysis } from "./screens-support";

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

test("large PR warns in the check card and starts only on Start analysis anyway", async ({ page }) => {
  await page.goto("/?scenario=large-pr");
  await analyze(page);
  await expect(page.getByRole("note")).toContainText("Large PR: 27 081 changed lines in 371 files.");
  await expect(page.getByRole("note")).toContainText("rough estimate from changed lines in recent runs");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  // Nothing has started yet; an edited URL clears the card, so its warning and Start never apply to another PR.
  await expect(page).toHaveURL(/\/\?scenario=large-pr$/);
  const field = page.getByLabel("GitHub pull request URL");
  await field.fill("https://github.com/acme/other/pull/1");
  await expect(page.getByRole("note")).toHaveCount(0);
  await expect(page.getByRole("region", { name: "Pull request check" })).toHaveCount(0);
  await field.fill(PR_URL);
  await expect(page.getByRole("region", { name: "Pull request check" })).toHaveCount(0);
  await analyze(page);
  await startAnalysis(page, "Start analysis anyway");
  await expect(page).toHaveURL(new RegExp(`/analyses/${ANALYSIS_ID}/progress`));
});

test("without history the large-PR note makes no timing claim", async ({ page }) => {
  await page.goto("/?scenario=large-pr-no-history");
  await analyze(page);
  await expect(page.getByRole("note")).toContainText("No time estimate yet.");
  await expect(page.getByRole("note")).not.toContainText("Based on recent runs");
  await expect(page.getByRole("button", { name: "Start analysis anyway", exact: true })).toBeEnabled();
});
