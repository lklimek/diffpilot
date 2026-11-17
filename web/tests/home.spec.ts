import { expect, test } from "@playwright/test";
import { analyze, expectAccessible, expectTooltips, PR_URL } from "./screens-support";
import { ANALYSIS_ID, expectTrustWording } from "./support";

const PROGRESS = new RegExp(`/analyses/${ANALYSIS_ID}/progress$`);

test("sign in → analyze → progress → open review", async ({ page }) => {
  await page.goto("/?scenario=signed-out");
  const signIn = page.getByRole("link", { name: "Sign in with GitHub" });
  await expect(signIn).toHaveAttribute("href", "/auth/github/login");
  await expectTrustWording(page);
  await expectAccessible(page);

  // The OAuth callback redirects to Home signed in (the mocks have no GitHub).
  await page.goto("/");
  await expect(page.getByRole("banner")).toContainText("alice");
  await expectTooltips(page);
  await analyze(page);
  await expect(page).toHaveURL(PROGRESS);
  await expect(page.getByText(/flows analysed|Planning flows…/)).toBeVisible();
  await expectTrustWording(page);
  await expect(page.getByText("5 of 5 flows analysed")).toBeVisible({ timeout: 10_000 });
  await page.getByRole("link", { name: "Open review" }).click();
  await expect(page).toHaveURL(new RegExp(`/analyses/${ANALYSIS_ID}#pr_context$`));
  await expect(page.locator("main h2:not([data-column-title])").first()).toHaveText(
    "PR context: acme/node-client #482",
  );
});

test("signed in: URL form, example and side card use the width at 3440", async ({ page }) => {
  await page.setViewportSize({ width: 3440, height: 1440 });
  await page.goto("/");
  const field = page.getByLabel("GitHub pull request URL");
  const aside = page.getByRole("complementary", { name: "What happens after Analyze" });
  await expect(aside).toBeVisible();
  const [fieldBox, asideBox] = [await field.boundingBox(), await aside.boundingBox()];
  expect(fieldBox!.width).toBeGreaterThan(1800);
  expect(asideBox!.x + asideBox!.width).toBeGreaterThan(3300);
  await expect(page.getByText("Example: https://github.com/owner/repo/pull/123")).toBeVisible();
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

for (const [url, message] of [
  ["https://github.com/acme/node-client/issues/5", "This is not a GitHub pull request URL."],
  ["https://github.com/acme/private-repo/pull/1", "This repository is private."],
  ["https://github.com/acme/node-client/pull/9", "This pull request does not exist"],
] as const) {
  test(`error: ${url}`, async ({ page }) => {
    await page.goto("/");
    await analyze(page, url);
    const alert = page.getByRole("alert");
    await expect(alert).toContainText("Cannot open this pull request.");
    await expect(alert).toContainText(message);
    await expect(page).toHaveURL(/\/$/);
    await expectTrustWording(page);
  });
}

test("existing analysis: Open review opens the review", async ({ page }) => {
  await page.goto("/?state=resume");
  await analyze(page);
  const banner = page.getByRole("status").filter({ hasText: "Analysis found." });
  await expect(banner).toContainText("This PR head is already analysed. Open review.");
  await expectTooltips(page);
  await banner.getByRole("link", { name: "Open review" }).click();
  await expect(page).toHaveURL(new RegExp(`/analyses/${ANALYSIS_ID}#f1\\.5$`));
});

test("head moved: banner, Re-analyze starts a fresh analysis", async ({ page }) => {
  await page.goto("/?scenario=head-moved");
  await analyze(page);
  const banner = page.getByRole("alert").filter({ hasText: "The PR has moved." });
  await expect(banner).toContainText("This analysis is for 52d2da1; the PR is now at d41b7a0.");
  await expect(banner.getByRole("link", { name: "Open the old analysis" })).toHaveAttribute(
    "href",
    `/analyses/${ANALYSIS_ID}`,
  );
  await expectTooltips(page);
  await expectAccessible(page);
  await banner.getByRole("button", { name: "Re-analyze" }).click();
  await expect(page).toHaveURL(PROGRESS);
});

test("analysis in progress and failed analysis lead to the progress screen", async ({ page }) => {
  await page.goto("/?scenario=existing-running");
  await analyze(page);
  await page.getByRole("link", { name: "Show progress" }).click();
  await expect(page).toHaveURL(PROGRESS);
  await expect(page.getByText("2 of 5 flows analysed")).toBeVisible();

  await page.goto("/?scenario=existing-failed");
  await analyze(page);
  await expect(page.getByRole("alert")).toContainText(
    "The analysis stopped because of a server error. Retry the analysis.",
  );
  await page.getByRole("link", { name: "Show details and retry" }).click();
  await expect(page.getByRole("button", { name: "Retry" })).toBeVisible();
});

test("without the GitHub App: warning, Analyze without it", async ({ page }) => {
  await page.goto("/?scenario=no-app");
  await analyze(page);
  const banner = page
    .getByRole("alert")
    .filter({ hasText: "The GitHub App is not installed on acme/node-client." });
  await expect(banner).toContainText("comments cannot be written");
  await banner.getByRole("button", { name: "Analyze without it" }).click();
  await expect(page).toHaveURL(PROGRESS);
});

test("cancelled: Home says 'Analysis cancelled — analyze again' with the URL filled in", async ({ page }) => {
  await page.goto(`/analyses/${ANALYSIS_ID}/progress?scenario=running`);
  await page.getByRole("button", { name: "Cancel analysis" }).click();
  await page.getByRole("button", { name: "Confirm cancel", exact: true }).click();
  await expect(page).toHaveURL(/\/$/);
  await expect(
    page.getByRole("status").filter({ hasText: "Analysis cancelled — analyze again." }),
  ).toBeVisible();
  await expect(page.getByLabel("GitHub pull request URL")).toHaveValue(PR_URL);
  await expectTrustWording(page);
});
