import { expect, test, type Page } from "@playwright/test";
import { expectAccessible, expectKeyboardTooltips, PR_URL } from "./screens-support";
import { ANALYSIS_ID } from "./support";

// My work at the dark route `/my-work` (plan §F, §H.2.2) against the msw dashboard scenarios
// (`?dashboard=<name>`, src/mocks/handlers/dashboard.ts).

/** One step of the mock's scripted refresh (`SCRIPT_STEP_MS` of src/mocks/handlers/dashboard.ts). */
const SCRIPT_STEP_MS = 500;

test.describe("My work (msw)", () => {
  const open = async (page: Page, scenario = "normal", extra = "") => {
    await page.goto(`/my-work?dashboard=${scenario}${extra}`);
    await expect(page.getByRole("heading", { name: "My work", level: 1 })).toBeVisible();
  };
  const main = (page: Page) => page.getByRole("main");

  test("normal: shell, ranked list opened on pull requests, side panel, keyboard tooltips", async ({
    page,
  }) => {
    await open(page);
    await expect(
      page.getByRole("navigation", { name: "Main" }).getByRole("link", { name: /My work/ }),
    ).toHaveAttribute("aria-current", "page");
    await expect(page.locator("[data-nav-count]")).toHaveText("14");
    await expect(main(page).getByText("fix(sync): retry header fetch after peer timeout")).toBeVisible();
    // Opened on pull requests (TODOs listed too): an issue that is not a P0 alarm is not shown.
    await expect(main(page).getByText("Wallet balance shows a stale value after a reorg")).toHaveCount(0);
    await expect(main(page).getByText("Write the migration note for config v3")).toBeVisible();
    await expect(page.locator("[data-scan-info]")).toHaveText(/Refreshed manually at/);
    // The live region says the state only, not the ticking "… ago".
    await expect(page.getByRole("status").filter({ hasText: /^Refreshed at \d\d:\d\d\.$/ })).toBeAttached();
    await expectKeyboardTooltips(page);
  });

  test("refreshing: the scan's progress and Cancel scan keep the earlier snapshot", async ({ page }) => {
    await open(page, "refreshing");
    await expect(page.getByRole("progressbar", { name: "Scan progress" })).toHaveAttribute(
      "aria-valuenow",
      "45",
    );
    await expect(page.getByRole("button", { name: /Refreshing/ })).toHaveAttribute("aria-disabled", "true");
    await page.getByRole("button", { name: "Cancel scan" }).click();
    await expect(page.getByRole("progressbar", { name: "Scan progress" })).toHaveCount(0);
    await expect(main(page).getByText("fix(sync): retry header fetch after peer timeout")).toBeVisible();
  });

  test("Refresh: scan, provisional ranking, then Ranking finished (mock clock)", async ({ page }) => {
    await page.clock.install();
    await open(page);
    await pauseClock(page);
    await page.getByRole("button", { name: "Refresh" }).click();
    await settle(page, (quick) =>
      quick(page.getByRole("progressbar", { name: "Scan progress" })).toBeVisible(),
    );
    // Into the scripted ranking (steps 5–6); time stands still there until the next jump.
    await page.clock.runFor(5.5 * SCRIPT_STEP_MS);
    await settle(page, async (quick) => {
      await quick(page.getByText(/Ranking with the LLM: \d+ of 14 items judged\./)).toBeVisible();
      await quick(main(page).getByText("Not judged yet", { exact: true })).toBeVisible();
    });
    await page.clock.runFor(2 * SCRIPT_STEP_MS);
    await settle(page, async (quick) => {
      await quick(page.getByText("Ranking finished: 14 items judged.")).toBeVisible();
      await quick(page.getByText(/Ranking with the LLM/)).toHaveCount(0);
    });
  });

  test("provisional and waiting for the review analysis", async ({ page }) => {
    await open(page, "provisional");
    await expect(page.getByText("Ranking with the LLM: 10 of 14 items judged.")).toBeVisible();
    await expect(main(page).getByText("fix(ui): restore the focus ring on tabs")).toBeVisible();
    await open(page, "waiting");
    await expect(page.getByText("Waiting for the review analysis to finish.")).toBeVisible();
  });

  test("a failed ranking keeps the estimates; Retry ranking ranks again (mock clock)", async ({ page }) => {
    await page.clock.install();
    await open(page, "ranking-failed");
    await expect(
      page.getByText("LLM ranking failed: The LLM stopped answering. Provisional scores stay approximate."),
    ).toBeVisible();
    await pauseClock(page);
    await page.getByRole("button", { name: "Retry ranking" }).click();
    await settle(page, (quick) => quick(page.getByText(/Ranking with the LLM/)).toBeVisible());
    await page.clock.runFor(2.5 * SCRIPT_STEP_MS);
    await settle(page, (quick) => quick(page.getByText("Ranking finished: 14 items judged.")).toBeVisible());
  });

  test("error: the rate limit stops the refresh; the 08:12 snapshot stays and Refresh waits", async ({
    page,
  }) => {
    await open(page, "error");
    await expect(
      page.getByRole("alert").filter({ hasText: "GitHub rate limit reached; the refresh stopped." }),
    ).toBeVisible();
    await expect(page.getByRole("button", { name: /^Retry at / }).first()).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    await expect(main(page).getByText(/^Previous snapshot · \d\d:\d\d$/)).toBeVisible();
    await expect(main(page).getByText("It is replaced when a scan finishes.")).toBeVisible();
  });

  test("empty: nothing needs you; your PRs waiting on others stay in the side panel", async ({ page }) => {
    await open(page, "empty");
    await expect(main(page).getByText("Nothing needs you right now.")).toBeVisible();
    await expect(page.locator("[data-nav-count]")).toHaveText("0");
  });

  test("first run: nothing scanned yet, then Refresh starts the first scan", async ({ page }) => {
    await open(page, "first-run");
    await expect(page.getByText("Nothing scanned yet.")).toBeVisible();
    await page.getByRole("button", { name: "Refresh" }).click();
    await expect(page.getByRole("progressbar", { name: "Scan progress" })).toBeVisible();
    await expect(main(page).getByText("fix(sync): retry header fetch after peer timeout")).toBeVisible({
      timeout: 10_000,
    });
  });

  test.describe("first visit", () => {
    test.use({ timezoneId: "Europe/Lisbon" });

    test("stores the browser's time zone once (R6b); the schedule note goes", async ({ page }) => {
      const saved = page.waitForRequest(
        (r) => r.method() === "PUT" && r.url().endsWith("/api/v1/settings/dashboard/time-zone"),
      );
      await open(page, "no-time-zone");
      expect((await saved).postDataJSON()).toEqual({ time_zone: "Europe/Lisbon" });
      await expect(page.getByText(/starts once your time zone is known/)).toHaveCount(0);
    });

    test("a refused time zone: the note points to Settings instead", async ({ page }) => {
      await open(page, "no-time-zone-refused");
      await expect(
        page.getByText("Your browser's time zone could not be saved; set it in Settings."),
      ).toBeVisible();
      await expect(page.getByText(/This page saves your browser's time zone/)).toHaveCount(0);
    });
  });

  test("1600×900, side panel open: headline beside its chart, first ranked row above the fold", async ({
    page,
  }) => {
    await page.setViewportSize({ width: 1600, height: 900 });
    await open(page);
    await expect(page.getByRole("separator", { name: "Resize the side panel" })).toBeVisible();
    const hero = (await page.locator("[data-headline-value]").boundingBox())!;
    const chart = (await page.getByRole("img", { name: /^Net closed minus opened/ }).boundingBox())!;
    expect(chart.x).toBeGreaterThan(hero.x + hero.width);
    const first = (await page.locator("li[data-item]").first().boundingBox())!;
    expect(first.y + first.height).toBeLessThanOrEqual(900);
  });

  test("signed out: the sign-in card", async ({ page }) => {
    await page.goto("/my-work?scenario=signed-out");
    await expect(page.getByRole("link", { name: "Sign in with GitHub" })).toHaveAttribute(
      "href",
      "/auth/github/login",
    );
  });

  for (const colorScheme of ["light", "dark"] as const) {
    test(`no serious axe violations in every mockup state, ${colorScheme}`, async ({ page }) => {
      await page.emulateMedia({ colorScheme });
      for (const scenario of [
        "normal",
        "refreshing",
        "provisional",
        "waiting",
        "ranking-failed",
        "empty",
        "error",
        "first-run",
        "no-time-zone",
        "no-time-zone-refused",
      ]) {
        await test.step(scenario, async () => {
          await open(page, scenario);
          await expectAccessible(page);
        });
      }
    });

    test(`no serious axe violations at 1600 px: side panel open, a row expanded, chart table, ${colorScheme}`, async ({
      page,
    }) => {
      await page.setViewportSize({ width: 1600, height: 900 });
      await page.emulateMedia({ colorScheme });
      await open(page);
      await expect(page.getByRole("separator", { name: "Resize the side panel" })).toBeVisible();
      await page.locator("li[data-item] button[aria-expanded]").first().click();
      const closed = page.locator("aside details:not([open]) > summary");
      while ((await closed.count()) > 0) await closed.first().click();
      await page.getByRole("button", { name: "Show as table" }).click();
      await expect(page.getByRole("region", { name: "Net per day as a table" })).toBeVisible();
      await expectAccessible(page);
    });
  }
});

/** Stops the page's clock shortly ahead: from now on, time moves only by `runFor`. */
async function pauseClock(page: Page): Promise<void> {
  const now = await page.evaluate(() => Date.now());
  await page.clock.pauseAt(now + 100);
}

/**
 * With the clock paused, fires the timers due now (TanStack Query batches its updates with `setTimeout(0)`)
 * until `check` passes with short assertions; time itself does not move.
 */
async function settle(page: Page, check: (quick: typeof expect) => Promise<void>): Promise<void> {
  const quick = expect.configure({ timeout: 200 });
  await expect(async () => {
    await page.clock.runFor(0);
    await check(quick);
  }).toPass({ timeout: 10_000, intervals: [50] });
}

test.describe("review start from a PR row (msw)", () => {
  const row = (page: Page) =>
    page.getByRole("listitem").filter({ hasText: "feat(rpc): retry with backoff on timeout" });

  test("new: Review opens the review start page with the PR; Check offers Start analysis", async ({
    page,
  }) => {
    await page.goto("/my-work?dashboard=review-new");
    await row(page).getByRole("link", { name: "Review" }).click();
    await expect(page.getByLabel(/pull request URL/i)).toHaveValue(PR_URL);
    await page.getByRole("button", { name: "Check" }).click();
    await expect(page.getByRole("button", { name: "Start analysis", exact: true })).toBeVisible();
  });

  test("found: the row opens the review of the current head", async ({ page }) => {
    await page.goto("/my-work?dashboard=review-found");
    await row(page).getByRole("link", { name: "Resume review" }).click();
    await expect(page).toHaveURL(new RegExp(`/analyses/${ANALYSIS_ID}`));
  });

  test("moved: the review start page says the PR has moved", async ({ page }) => {
    await page.goto("/my-work?dashboard=review-moved");
    await row(page).getByRole("link", { name: "Open review" }).click();
    await expect(page.getByLabel(/pull request URL/i)).toHaveValue(PR_URL);
    await page.getByRole("button", { name: "Check" }).click();
    await expect(page.getByText("The PR has moved.")).toBeVisible();
  });
});
