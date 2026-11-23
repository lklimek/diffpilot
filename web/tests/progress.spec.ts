import { type Page } from "@playwright/test";
import { PINNED_NOW, expect, test } from "./fixtures";
import { expectAccessible, expectTooltips } from "./screens-support";
import { ANALYSIS_ID, expectTrustWording } from "./support";

const open = (page: Page, scenario: string) =>
  page.goto(`/analyses/${ANALYSIS_ID}/progress?scenario=${scenario}`);
const bar = (page: Page) => page.getByRole("progressbar", { name: "Analysis progress" });
const openReview = (page: Page) => page.getByRole("link", { name: "Open", exact: true });

test("planning: indeterminate bar, Planning flows…, Cancel analysis, no Open", async ({ page }) => {
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

test("running: k of n, no Open", async ({ page }) => {
  await open(page, "running");
  await expect(page.getByText("2 of 5 flows analysed")).toBeVisible();
  await expect(bar(page)).toHaveAttribute("aria-valuenow", "2");
  await expect(openReview(page)).toHaveCount(0);
  await expectTrustWording(page);
});

test("paused: usage limit with resume time", async ({ page }) => {
  await page.clock.setFixedTime(new Date("2026-11-17T14:20:00Z"));
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

test("finished: n of n and Open", async ({ page }) => {
  await open(page, "finished");
  await expect(page.getByText("5 of 5 flows analysed")).toBeVisible();
  await expect(page.getByRole("button", { name: "Cancel analysis" })).toHaveCount(0);
  await expect(openReview(page)).toHaveAttribute("href", `/analyses/${ANALYSIS_ID}`);
  await expect(openReview(page)).toHaveAccessibleDescription(/Starts nothing/);
  await expect(page.getByRole("group", { name: "Analysis actions" }).getByRole("link")).toHaveCount(0);
  await expectTooltips(page);
  await expectAccessible(page);
});

test("finished: Open stays on screen at narrow widths", async ({ page }) => {
  for (const width of [600, 800, 1024]) {
    await page.setViewportSize({ width, height: 800 });
    await open(page, "finished");
    const box = await openReview(page).boundingBox();
    expect(box).not.toBeNull();
    expect(box!.x).toBeGreaterThanOrEqual(0);
    expect(box!.x + box!.width).toBeLessThanOrEqual(width);
  }
});

test("unknown analysis: plain error and a way back", async ({ page }) => {
  await page.goto("/analyses/00000000-0000-0000-0000-000000000000/progress");
  await expect(page.getByRole("alert")).toContainText("This analysis does not exist.");
  await expect(page.getByRole("link", { name: "My work" })).toHaveAttribute("href", "/");
});

// Owner: a one-flow PR stayed at "0 of 1 flow analysed" for minutes and looked frozen. Stage and heartbeat come
// from the server (sse-events.md); the mocks send a heartbeat every 15 s.
test.describe("stage and liveness", () => {
  const indicator = (page: Page) => page.locator("[data-activity]");
  // The part of the bar still to do: it moves only while a job works.
  const rest = (page: Page) => bar(page).locator(":scope > span");

  test("the stage line follows the server: planning, PR context, flow 1 with its title; time is real", async ({
    page,
  }) => {
    await open(page, "first-flow");
    await expect(page.getByText("Planning flows…")).toBeVisible();
    await expect(page.getByText("Reading the PR context.")).toBeVisible();
    await expect(page.getByText("0 of 1 flow analysed")).toBeVisible();
    // The first heartbeat gives the server's clock; only then is a time shown.
    await expect(indicator(page)).toHaveText("Working");
    await page.clock.runFor(16_000);
    await expect(indicator(page)).toHaveText("Working · this stage started less than a minute ago");
    await page.clock.runFor(15_000);
    await expect(page.getByText("Analysing flow 1 of 1: Send an RPC request with retry")).toBeVisible();
    await expect(bar(page)).toHaveAttribute("aria-valuenow", "0");
    await expect(rest(page)).not.toHaveCSS("animation-name", "none");
    await expectTrustWording(page);
    await expectAccessible(page);
  });

  test("a long flow with heartbeats stays Working", async ({ page }) => {
    await open(page, "running");
    await expect(page.getByText(/^Analysing flow 3 of 5: /)).toBeVisible();
    for (let i = 0; i < 41; i += 1) await page.clock.runFor(15_000);
    await expect(indicator(page)).toHaveText("Working · this stage started 10 min ago");
    await expect(page.getByText(/No signal from the server/)).toHaveCount(0);
  });

  test("waiting for the runner, and between two jobs (not idle)", async ({ page }) => {
    await open(page, "waiting");
    await expect(page.getByText("Waiting for the runner; next: planning the review.")).toBeVisible();
    await expect(indicator(page)).toHaveCount(0);
    await expectTrustWording(page);
    await open(page, "between-jobs");
    await expect(page.getByText("Between two jobs: the server starts the next one.")).toBeVisible();
    await expect(indicator(page)).toHaveCount(0);
  });

  test("no heartbeat: the notice says so with the real time and the indicator stops", async ({ page }) => {
    await open(page, "signal-lost");
    await expect(indicator(page)).toBeVisible();
    await page.clock.runFor(60_000);
    await expect(page.getByText(/^No signal from the server for \d+ s; reconnecting…$/)).toBeVisible();
    await expect(indicator(page)).toHaveCount(0);
    await expect(rest(page)).toHaveCSS("animation-name", "none");
    await expect(page.getByText(/^Analysing flow 3 of 5: /)).toBeVisible();
    await expectTrustWording(page);
    await expectAccessible(page);
  });

  test("reduced motion: the indicator and the bar do not move and Working is said in text", async ({
    page,
  }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await open(page, "running");
    await expect(indicator(page)).toContainText("Working");
    const dot = indicator(page).locator("span[aria-hidden]");
    await expect(dot).toHaveCSS("animation-name", "none");
    await expect(rest(page)).toHaveCSS("animation-name", "none");
    await page.emulateMedia({ reducedMotion: "no-preference" });
    await expect(dot).not.toHaveCSS("animation-name", "none");
    await expect(rest(page)).not.toHaveCSS("animation-name", "none");
  });

  // Light and dark at 1440 with SCREENSHOT_DIR set, e.g. /data/artifacts/diffpilot/<date>/wp3-tour-9.
  for (const colorScheme of ["light", "dark"] as const) {
    test(`screenshots ${colorScheme} 1440`, async ({ browser }) => {
      const dir = process.env.SCREENSHOT_DIR;
      test.skip(!dir, "SCREENSHOT_DIR not set");
      const page = await browser.newPage({ colorScheme, viewport: { width: 1440, height: 900 } });
      await page.clock.install({ time: PINNED_NOW });
      const shoot = async (scenario: string, name: string, wait: number, expected: RegExp) => {
        await page.goto(`/analyses/${ANALYSIS_ID}/progress?scenario=${scenario}`);
        await expect(
          page.getByText(/^(Planning the review|Analysing flow|Waiting for the runner)/),
        ).toBeVisible();
        for (let t = 0; t < wait; t += 15_000) await page.clock.runFor(15_000);
        await expect(page.locator("section[aria-label='Analysis progress']")).toContainText(expected);
        await page.screenshot({ path: `${dir}/progress-${name}-${colorScheme}-1440.png` });
      };
      await shoot("planning", "planning", 90_000, /stage started 1 min ago/);
      await shoot("running", "long-flow", 615_000, /stage started 10 min ago/);
      await shoot("waiting", "waiting", 0, /Waiting for the runner/);
      await shoot("signal-lost", "signal-lost", 60_000, /No signal from the server/);
      await page.close();
    });
  }
});

test("plan parts retain the whole-run clock, counters and estimate", async ({ page }) => {
  await open(page, "plan-parts");
  await expect(page.getByText("Planning the review, part 6 of 23.")).toBeVisible();
  const stages = page.getByRole("list", { name: "Analysis stages" });
  await expect(stages).toContainText("Planning: 5 / 23 · current");
  await expect(stages).toContainText("PR context: 0 / 1 · pending");
  await expect(stages).toContainText("Flows: known after planning");
  await expect(stages).toContainText("Finishing: 0 / 1 · pending");
  await expect(
    page.getByText(/About 40 min left.*rough estimate from this analysis's finished parts/),
  ).toBeVisible();
  await expect(bar(page)).toHaveAttribute("aria-valuenow", "12");
  await page.clock.runFor(16_000);
  await expect(page.getByText("Analysis started 40 min ago")).toBeVisible();
  await expect(page.getByText(/this stage started 5 min ago/)).toBeVisible();
  await page.clock.runFor(46_000);
  await expect(page.getByText("Planning the review, part 7 of 23.")).toBeVisible();
  // The heartbeats fired inside runFor arrive (and set the server clock offset) only after it returns; one more
  // tick then reads the elapsed time with that offset, whatever the page's load time.
  await page.clock.runFor(10_000);
  await expect(page.getByText("Analysis started 41 min ago")).toBeVisible();
  await expect(page.getByText(/this stage started less than a minute ago/)).toBeVisible();
  await expect(stages).toContainText("Planning: 6 / 23 · current");
  await expectAccessible(page);
});

for (const [scenario, text] of [
  ["estimate-recent", "estimate from recent runs"],
  ["estimate-lines", "rough estimate from changed lines in recent runs"],
  ["estimate-none", "No estimate yet"],
] as const) {
  test(`waiting estimate: ${scenario}`, async ({ page }) => {
    await open(page, scenario);
    await expect(page.getByText(text, { exact: false })).toBeVisible();
    if (scenario === "estimate-none") await expect(bar(page)).not.toHaveAttribute("aria-valuenow", /.*/);
  });
}

test("paused run shows the resume time instead of an ETA", async ({ page }) => {
  await open(page, "estimate-paused");
  await expect(page.getByText(/^Paused until/)).toBeVisible();
  await expect(page.getByText(/About .* left|No estimate yet/)).toHaveCount(0);
  await expect(page.getByRole("list", { name: "Analysis stages" })).toBeVisible();
});

test("plan ready, nothing analysed by itself: says so, Open and Analyse all remaining flows", async ({
  page,
}) => {
  await open(page, "lazy-flows");
  await expect(
    page.getByText(
      "The plan is ready. Nothing is being analysed. Choose a flow to analyse, or analyse all remaining flows.",
    ),
  ).toBeVisible();
  await expect(openReview(page)).toBeVisible();
  await expectTooltips(page);
  await expect(page.getByText("4 not analysed")).toBeVisible();
  await page.getByRole("button", { name: "Analyse all remaining flows" }).click();
  await expect(page.getByRole("button", { name: "Analysing all remaining flows — Stop" })).toBeVisible();
  // The next flow by rank is queued: the stage line names it.
  await expect(page.getByText(/flow 2 of 5: /)).toBeVisible();
  await expectAccessible(page);
});
