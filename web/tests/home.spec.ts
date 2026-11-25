import { expect, test } from "./fixtures";
import { analyze, expectAccessible, expectTooltips, startAnalysis } from "./screens-support";
import { ANALYSIS_ID, expectTrustWording } from "./support";

// Starting an analysis opens the progress URL, which redirects to the PR page (review-workflow-plan §3.1).
const PROGRESS = /\/pr\/acme\/node-client\/482(?:\?|$)/;

test("sign in → analyze → progress → resume review", async ({ page }) => {
  await page.goto("/prs?scenario=signed-out");
  const signIn = page.getByRole("link", { name: "Sign in with GitHub" });
  await expect(signIn).toHaveAttribute("href", "/auth/github/login");
  await expectTrustWording(page);
  await expectAccessible(page);

  // The OAuth callback redirects to Home signed in (the mocks have no GitHub).
  await page.goto("/prs");
  await expect(page.getByRole("banner")).toContainText("alice");
  await expectTooltips(page);
  await analyze(page);
  await expect(page).toHaveURL(PROGRESS);
  const start = page.getByRole("button", { name: "Start analysis", exact: true });
  await expect(start).toBeVisible();
  await expect(page.getByRole("region", { name: "Analysis progress" })).toHaveCount(0);
  await start.focus();
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(PROGRESS);
  await expect(page.getByText(/flows analysed|Planning flows…/)).toBeVisible();
  await expectTrustWording(page);
  await expect(page.getByText("5 of 5 flows analysed")).toBeVisible({ timeout: 10_000 });
  await page.getByRole("link", { name: "Open review", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/analyses/${ANALYSIS_ID}#pr_context$`));
  await expect(page.locator("main h2:not([data-column-title])").first()).toHaveText(
    "PR context: acme/node-client #482",
  );
});

test("signed in: the opener and history fit at 3440", async ({ page }) => {
  await page.setViewportSize({ width: 3440, height: 1440 });
  await page.goto("/prs");
  const field = page.getByLabel("GitHub pull request URL");
  await expect(field).toHaveAttribute("placeholder", /owner\/repo#123/);
  expect((await field.boundingBox())!.width).toBeGreaterThan(750);
  await expectAccessible(page);
});

for (const [code, title] of [
  ["login_not_allowed", "This GitHub login is not allowed."],
  ["oauth_failed", "Sign-in with GitHub did not complete."],
  ["state_mismatch", "The sign-in expired."],
] as const) {
  test(`login error ${code}`, async ({ page }) => {
    await page.goto(`/?scenario=signed-out&login_error=${code}`);
    await expect(page.getByRole("alert")).toContainText(title);
    await expect(page.getByRole("link", { name: "Sign in with GitHub" })).toBeVisible();
    await expectTrustWording(page);
  });
}

for (const [url, message, path] of [
  ["https://github.com/acme/node-client/issues/5", "Enter a GitHub pull request link", "/prs"],
  ["https://github.com/acme/private-repo/pull/1", "This repository is private.", "/pr/acme/private-repo/1"],
  [
    "https://github.com/acme/node-client/pull/9",
    "This pull request is not known to diffpilot.",
    "/pr/acme/node-client/9",
  ],
] as const) {
  test(`error: ${url}`, async ({ page }) => {
    await page.goto("/prs");
    await analyze(page, url);
    const alert = page.getByRole("alert");
    await expect(alert).toContainText(message);
    await expect(page).toHaveURL(path);
    await expectTrustWording(page);
  });
}

test("existing analysis: Open review opens the review", async ({ page }) => {
  await page.goto("/prs?state=resume");
  await analyze(page);
  const banner = page.getByRole("status").filter({ hasText: "Analysis found." });
  await expect(banner).toContainText("This PR head is already analysed.");
  await expectTooltips(page);
  await banner.getByRole("link", { name: "Open review", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/analyses/${ANALYSIS_ID}#f1\\.5$`));
});

test("head moved: banner, Start analysis starts a fresh analysis", async ({ page }) => {
  await page.goto("/prs?scenario=head-moved");
  await analyze(page);
  const banner = page.getByRole("alert").filter({ hasText: "The PR has moved." });
  await expect(banner).toContainText("Analysis of 52d2da1; the PR is now at d41b7a0.");
  await expect(banner.getByRole("link", { name: "Open the old analysis" })).toHaveAttribute(
    "href",
    `/analyses/${ANALYSIS_ID}`,
  );
  await expectTooltips(page);
  await expectAccessible(page);
  await startAnalysis(page, "Start analysis of the new head");
  await expect(page).toHaveURL(PROGRESS);
});

test("live progress and a failed analysis are shown on the PR page", async ({ page }) => {
  await page.goto("/prs?scenario=existing-running");
  await analyze(page);
  await expect(page).toHaveURL(PROGRESS);
  await expect(page.getByText("2 of 5 flows analysed")).toBeVisible();

  await page.goto("/prs?scenario=existing-failed");
  await analyze(page);
  await expect(page.getByRole("region", { name: "Analysis progress" })).toContainText(
    "The analysis stopped because of a server error. Retry the analysis.",
  );
  await expect(page.getByRole("button", { name: "Retry" })).toBeVisible();
});

test("without the GitHub App: warning, Start analysis anyway", async ({ page }) => {
  await page.goto("/prs?scenario=no-app");
  await analyze(page);
  const banner = page
    .getByRole("alert")
    .filter({ hasText: "Commenting is not available for acme/node-client right now." });
  await expect(banner).toContainText("comments cannot be written");
  await startAnalysis(page, "Start analysis anyway");
  await expect(page).toHaveURL(PROGRESS);
});

test("cancelled analysis stays on the PR page with an explicit way to start again", async ({ page }) => {
  await page.goto(`/analyses/${ANALYSIS_ID}/progress?scenario=running`);
  await page.getByRole("button", { name: "Cancel analysis" }).click();
  await page.getByRole("button", { name: "Confirm cancel", exact: true }).click();
  await expect(page).toHaveURL(PROGRESS);
  await expect(page.getByRole("status").filter({ hasText: "Analysis cancelled." })).toBeVisible();
  await expect(page.getByRole("button", { name: "Start analysis", exact: true })).toBeEnabled();
  await expectTrustWording(page);
});

test("empty history offers a link to My work", async ({ page }) => {
  await page.goto("/prs?scenario=empty-pr-history");
  const section = page.getByRole("region", { name: "Your pull requests" });
  await expect(section.getByText(/No pull requests yet/)).toBeVisible();
  await expect(section.getByRole("link", { name: "My work" })).toHaveAttribute("href", "/");
  await expectAccessible(page);
});
