import { expect, test, type Page } from "@playwright/test";
import { expectAccessible, expectKeyboardTooltips, PR_URL } from "./screens-support";
import { ANALYSIS_ID } from "./support";

// My work at the dark route `/my-work` (plan §F, §H.2.2) against the msw dashboard scenarios
// (`?dashboard=<name>`, src/mocks/handlers/dashboard.ts).

test.describe("My work (msw)", () => {
  // TODO(WP11-WEB-S after WEB-C merge): the screen arranges the WEB-C components (components/app-shell,
  // components/dashboard); remove this fixme once they are merged.
  test.fixme(true, "needs the WP11-WEB-C components");

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
    await expect(page.getByRole("status").filter({ hasText: /Refreshed manually at/ })).toBeVisible();
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

  test("Refresh: scan, provisional ranking, then Ranking finished", async ({ page }) => {
    await open(page);
    await page.getByRole("button", { name: "Refresh" }).click();
    await expect(page.getByRole("progressbar", { name: "Scan progress" })).toBeVisible();
    await expect(page.getByText(/Ranking with the LLM: \d+ of 14 items judged\./)).toBeVisible({
      timeout: 10_000,
    });
    await expect(main(page).getByText("Not judged yet", { exact: true })).toBeVisible();
    await expect(page.getByText("Ranking finished: 14 items judged.")).toBeVisible({ timeout: 10_000 });
    await expect(page.getByText(/Ranking with the LLM/)).toHaveCount(0);
  });

  test("provisional and waiting for the review analysis", async ({ page }) => {
    await open(page, "provisional");
    await expect(page.getByText("Ranking with the LLM: 10 of 14 items judged.")).toBeVisible();
    await expect(main(page).getByText("fix(ui): restore the focus ring on tabs")).toBeVisible();
    await open(page, "waiting");
    await expect(page.getByText("Waiting for the review analysis to finish.")).toBeVisible();
  });

  test("a failed ranking keeps the estimates; Retry ranking ranks again", async ({ page }) => {
    await open(page, "ranking-failed");
    await expect(page.getByText(/LLM ranking failed:/)).toBeVisible();
    await page.getByRole("button", { name: "Retry ranking" }).click();
    await expect(page.getByText(/Ranking with the LLM/)).toBeVisible();
    await expect(page.getByText("Ranking finished: 14 items judged.")).toBeVisible({ timeout: 10_000 });
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
    await expect(main(page).getByText(/snapshot\. It is replaced when a scan finishes\./)).toBeVisible();
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
      for (const scenario of ["normal", "refreshing", "provisional", "waiting", "empty", "error"]) {
        await test.step(scenario, async () => {
          await open(page, scenario);
          await expectAccessible(page);
        });
      }
    });
  }
});

test.describe("review start from a PR row (msw)", () => {
  // TODO(WP11-WEB-S after WEB-C merge): remove this fixme once the WP11-WEB-C components are merged.
  test.fixme(true, "needs the WP11-WEB-C components");

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
