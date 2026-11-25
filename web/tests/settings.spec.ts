import { expect, test } from "./fixtures";
import { expectAccessible, expectTooltips } from "./screens-support";
import { expectTrustWording } from "./support";

test("old links lead to the corresponding settings pages and show the repository rules", async ({ page }) => {
  await page.goto("/settings");
  await expect(page).toHaveURL(/\/settings\/account$/);
  const nav = page.getByRole("navigation", { name: "Main" });
  await expect(nav.getByRole("link", { name: "Settings", exact: true })).not.toHaveAttribute(
    "aria-current",
    "page",
  );
  await expect(nav.getByRole("link", { name: "Account" })).toHaveAttribute("aria-current", "page");
  await nav.getByRole("button", { name: "Collapse navigation" }).click();
  await expect(nav.getByRole("link", { name: "LLM & usage" })).toBeVisible();
  await nav.getByRole("link", { name: "LLM & usage" }).press("Enter");
  await expect(page.getByRole("region", { name: "Last LLM call" })).toContainText("Succeeded");
  await page.goto("/settings?repo=acme%2Fnode-client");
  await expect(page).toHaveURL(/\/settings\/repositories\?repo=acme%2Fnode-client$/);
  const repo = page.getByRole("region", { name: "Analysis rules for acme/node-client" });
  await repo.getByText(/Show the .* built-in rules/).click();
  await expect(repo.getByRole("cell", { name: "tests/**" })).toBeVisible();
  await expect(repo).toContainText("acme/node-sdk");
  await expectTooltips(page);
  await expectTrustWording(page);
  await expectAccessible(page);
});

test("GitHub App shows the configured webhook URL and copies it", async ({ page, context }) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.goto("/settings/github-sync");
  const app = page.getByRole("region", { name: "GitHub App" });
  await expect(app).toContainText("https://webhooks.manago.cloud/diffpilot/github");
  await app.getByRole("button", { name: "Copy" }).click();
  await expect(app.getByRole("status")).toHaveText("Copied");
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(
    "https://webhooks.manago.cloud/diffpilot/github",
  );
});

test("language change is saved", async ({ page }) => {
  await page.goto("/settings/llm");
  const select = page.getByLabel(/written in/);
  await select.selectOption("pl");
  await expect(select).toHaveValue("pl");
  await expect(select).toBeEnabled();
  await select.selectOption("ru");
  await expect(select).toHaveValue("ru");
  await expect(select).toBeEnabled();
});

test("LLM not working: plain message", async ({ page }) => {
  await page.goto("/settings/llm?scenario=llm-error");
  const llm = page.getByRole("region", { name: "Last LLM call" });
  await expect(llm.getByText(/^Failed/)).toBeVisible();
  await expect(llm).toContainText("The LLM provider's usage limit was reached.");
  await expectTrustWording(page);
  await expectAccessible(page);
});

test("GitHub App installations not checked", async ({ page }) => {
  await page.goto("/settings/github-sync?scenario=installations-not-checked");
  const app = page.getByRole("region", { name: "GitHub App" });
  await expect(app).toContainText("GitHub App installations: not checked. Signing in again may help.");
  await expect(app.getByRole("link", { name: "Sign in with GitHub" })).toHaveAttribute(
    "href",
    "/auth/github/login",
  );
  await expectTrustWording(page);
  await expectAccessible(page);
});

test("no LLM call yet, no profile, no related repositories", async ({ page }) => {
  await page.goto("/settings/llm?scenario=llm-unknown");
  await expect(page.getByRole("region", { name: "Last LLM call" })).toContainText("No LLM call yet.");
  await page.goto("/settings/repositories?scenario=llm-unknown&repo=acme%2Fnode-client");
  const repo = page.getByRole("region", { name: "Analysis rules for acme/node-client" });
  await expect(repo).toContainText("No project profile");
  await expect(repo).toContainText("None detected");
  await expectTrustWording(page);
});

test("no App installation", async ({ page }) => {
  await page.goto("/settings/github-sync?scenario=no-app");
  await expect(page.getByRole("region", { name: "GitHub App" })).toContainText(
    "not installed on any account",
  );
});

test("signed out: sign-in card", async ({ page }) => {
  await page.goto("/settings?scenario=signed-out");
  await expect(page.getByRole("link", { name: "Sign in with GitHub" })).toBeVisible();
  await expect(page.getByRole("region", { name: "Last LLM call" })).toHaveCount(0);
});

test("repository settings not known yet: a neutral note", async ({ page }) => {
  await page.goto("/settings/repositories?scenario=repo-not-known&repo=acme%2Fnode-client");
  await expect(
    page.getByText(
      "Not known yet: the repository settings are read during an analysis and appear once one has run.",
    ),
  ).toBeVisible();
  await expect(page.getByRole("alert")).toHaveCount(0);
});
