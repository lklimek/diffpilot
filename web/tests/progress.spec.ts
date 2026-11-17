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
  await expect(page.getByText(/^Paused until \d\d:\d\d — 2 of 5$/)).toBeVisible();
  await expect(bar(page)).toHaveAttribute("aria-valuenow", "2");
  await expect(openReview(page)).toHaveCount(0);
  await expectTrustWording(page);

  await open(page, "paused-unknown");
  await expect(page.getByText("Paused; the resume time is not known")).toBeVisible();
});

test("failed in a flow: names it; Retry runs the analysis to finished", async ({ page }) => {
  await open(page, "failed");
  await expect(page.getByText("Analysis of flow “Classify RPC errors” failed")).toBeVisible();
  await expect(page.getByText("The agent's answer did not have the expected structure.")).toBeVisible();
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
  await expect(page.getByText("The repository could not be fetched. Retry the analysis.")).toBeVisible();
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

// Owner: a one-flow PR stayed at "0 of 1 flows analysed" for minutes and looked frozen.
test.describe("activity while nothing is counted yet", () => {
  const indicator = (page: Page) => page.locator("[data-activity]");

  test("the screen changes during planning and before any flow is done; times are real; silence is said", async ({
    page,
  }) => {
    await page.clock.install();
    await open(page, "first-flow");
    await expect(page.getByText("Planning flows…")).toBeVisible();
    await expect(page.getByText("Planning the review: grouping the changes into flows.")).toBeVisible();
    // The plan arrives: the count is still 0, the stage line moves on.
    await expect(page.getByText("Reading the PR context, then analysing flow 1 of 1.")).toBeVisible();
    await expect(page.getByText("0 of 1 flows analysed")).toBeVisible();
    await expect(indicator(page)).toHaveText("Working · this stage started less than a minute ago");
    await expectTrustWording(page);
    await expectAccessible(page);

    await page.clock.fastForward("02:00");
    await expect(indicator(page)).toHaveText("Working · this stage started 2 min ago");

    await page.clock.fastForward("04:00");
    await expect(
      page.getByText("No update from the server for more than 5 min; the analysis may still be running."),
    ).toBeVisible();
    await expect(indicator(page)).toHaveCount(0);
    await expect(page.getByText("This stage started 6 min ago")).toBeVisible();
    await expectTrustWording(page);
    await expectAccessible(page);
  });

  test("reduced motion: the indicator does not move and says Working in text", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await open(page, "first-flow");
    await expect(indicator(page)).toContainText("Working");
    const dot = indicator(page).locator("span[aria-hidden]");
    await expect(dot).toHaveCSS("animation-name", "none");
    await page.emulateMedia({ reducedMotion: "no-preference" });
    await expect(dot).not.toHaveCSS("animation-name", "none");
  });

  // Light and dark at 1440 with SCREENSHOT_DIR set, e.g. /data/artifacts/smart-review/<date>/wp3-tour-7.
  for (const colorScheme of ["light", "dark"] as const) {
    test(`screenshots ${colorScheme} 1440`, async ({ browser }) => {
      const dir = process.env.SCREENSHOT_DIR;
      test.skip(!dir, "SCREENSHOT_DIR not set");
      const page = await browser.newPage({ colorScheme, viewport: { width: 1440, height: 900 } });
      await page.clock.install();
      await page.goto(`/analyses/${ANALYSIS_ID}/progress?scenario=planning`);
      await expect(indicator(page)).toBeVisible();
      await page.clock.fastForward("01:30");
      await expect(indicator(page)).toHaveText(/at least 1 min/);
      await page.screenshot({ path: `${dir}/progress-planning-${colorScheme}-1440.png` });
      await page.goto(`/analyses/${ANALYSIS_ID}/progress?scenario=first-flow`);
      await expect(page.getByText("Reading the PR context, then analysing flow 1 of 1.")).toBeVisible();
      await page.clock.fastForward("02:00");
      await expect(indicator(page)).toHaveText(/2 min ago/);
      await page.screenshot({ path: `${dir}/progress-first-flow-${colorScheme}-1440.png` });
      await page.clock.fastForward("04:00");
      await expect(page.getByText(/^No update from the server/)).toBeVisible();
      await page.screenshot({ path: `${dir}/progress-no-update-${colorScheme}-1440.png` });
      await page.close();
    });
  }
});
