import { expect, test } from "@playwright/test";
import { expectAccessible, expectTooltips } from "./screens-support";
import { expectTrustWording } from "./support";

test("default: LLM working with time, language, App, repository rules", async ({ page }) => {
  await page.goto("/settings");
  const llm = page.getByRole("region", { name: "LLM status" });
  await expect(llm.getByText("working", { exact: true })).toBeVisible();
  await expect(llm).toContainText("last LLM call:");
  await expect(page.getByLabel(/written in/)).toHaveValue("en");
  await expect(page.getByRole("region", { name: "GitHub App" })).toContainText("acme: selected repositories");
  const repo = page.getByRole("region", { name: /acme\/node-client/ });
  await repo.getByText(/Show the .* built-in rules/).click();
  await expect(repo.getByRole("cell", { name: "tests/**" })).toBeVisible();
  await expect(repo).toContainText("acme/node-sdk");
  await expect(page.getByRole("button", { name: /check/i })).toHaveCount(0);
  await expectTooltips(page);
  await expectTrustWording(page);
  await expectAccessible(page);
});

test("language change is saved", async ({ page }) => {
  await page.goto("/settings");
  const select = page.getByLabel(/written in/);
  await select.selectOption("pl");
  await expect(select).toHaveValue("pl");
  await expect(select).toBeEnabled();
});

test("LLM not working: plain message", async ({ page }) => {
  await page.goto("/settings?scenario=llm-error");
  const llm = page.getByRole("region", { name: "LLM status" });
  await expect(llm.getByText("not working")).toBeVisible();
  await expect(llm).toContainText("The LLM provider's usage limit was reached.");
  await expectTrustWording(page);
  await expectAccessible(page);
});

test("GitHub App installations not checked", async ({ page }) => {
  await page.goto("/settings?scenario=installations-not-checked");
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
  await page.goto("/settings?scenario=llm-unknown");
  await expect(page.getByRole("region", { name: "LLM status" })).toContainText("not known yet");
  const repo = page.getByRole("region", { name: /acme\/node-client/ });
  await expect(repo).toContainText("No project profile");
  await expect(repo).toContainText("None detected");
  await expectTrustWording(page);
});

test("no App installation", async ({ page }) => {
  await page.goto("/settings?scenario=no-app");
  await expect(page.getByRole("region", { name: "GitHub App" })).toContainText(
    "not installed on any account",
  );
});

test("signed out: sign-in card", async ({ page }) => {
  await page.goto("/settings?scenario=signed-out");
  await expect(page.getByRole("link", { name: "Sign in with GitHub" })).toBeVisible();
  await expect(page.getByRole("region", { name: "LLM status" })).toHaveCount(0);
});

test("repository settings not known yet: a neutral note", async ({ page }) => {
  await page.goto("/settings?scenario=repo-not-known");
  await expect(
    page.getByText(
      "Not known yet: the repository settings are read during an analysis and appear once one has run.",
    ),
  ).toBeVisible();
  await expect(page.getByRole("alert")).toHaveCount(0);
});
