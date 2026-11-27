import { expect, test } from "./fixtures";
import { expectAccessible } from "./screens-support";

test("history opens a PR first, with a separate action for finished reviews", async ({ page }) => {
  await page.goto("/prs");
  const row = page.getByRole("listitem").filter({ hasText: "acme/node-client #482" });
  const review = row.getByRole("link", { name: "Review acme/node-client #482", exact: true });
  await expect(review).toHaveAttribute("href", /\/analyses\//);
  const pr = row.getByRole("link", { name: /^acme\/node-client #482/ });
  await pr.focus();
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(/\/pr\/acme\/node-client\/482$/);
  await expect(page.getByRole("region", { name: "Review the code" })).toBeVisible();
  const crumbs = page.getByRole("navigation", { name: "Breadcrumb" });
  const prCrumb = crumbs.getByText("#482", { exact: true }).locator("..");
  await expect(prCrumb).toContainText("acme/node-client #482");
  await expect(prCrumb).toHaveAttribute("aria-current", "page");
  await expect(crumbs.getByRole("link", { name: "acme/node-client #482", exact: true })).toHaveCount(0);
});

test("history filters and watch menu work on a phone with a legacy server", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/watching?scenario=legacy-pr-history");
  await expect(page).toHaveURL(/\/prs\?.*show=watched/);
  await expect(page.getByRole("button", { name: "Watched" })).toHaveAttribute("aria-pressed", "true");
  await page.getByRole("searchbox", { name: "Filter by repository or title" }).fill("frame reader");
  const list = page.getByRole("list", { name: "Your pull requests" });
  await expect(list.getByRole("listitem")).toHaveCount(1);
  const more = list.getByRole("button", { name: /More actions/ });
  await more.focus();
  await page.keyboard.press("ArrowDown");
  await expect(page.getByRole("menuitem", { name: "Stop watching" })).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(more).toBeFocused();
  await expectAccessible(page);
  expect(
    await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth),
  ).toBeLessThanOrEqual(0);
});

test("the review breadcrumb shortens the label and always returns to the PR main page", async ({ page }) => {
  await page.goto("/prs?state=resume");
  const row = page.getByRole("listitem").filter({ hasText: "acme/node-client #482" });
  await row.getByRole("link", { name: "Review acme/node-client #482", exact: true }).click();
  const crumbs = page.getByRole("navigation", { name: "Breadcrumb" });
  const pr = crumbs.getByRole("link", { name: "acme/node-client #482" });
  await expect(crumbs.getByRole("link", { name: "Pull requests" })).toHaveAttribute("href", "/prs");
  await expect(pr).toHaveAttribute("href", "/pr/acme/node-client/482");
  const repo = pr.locator(".review-repo");
  const owner = repo.locator(".review-owner");
  const name = repo.locator(".review-repo-name");
  await expect(repo).toHaveText("acme/node-client");
  await expect(owner).toBeVisible();
  await expect(name).toBeVisible();
  await page.setViewportSize({ width: 1200, height: 900 });
  await expect(owner).toBeHidden();
  await expect(name).toHaveText("node-client");
  await expect(name).toBeVisible();
  await expect(pr).toHaveAccessibleDescription("acme/node-client #482");
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(repo).toBeHidden();
  await expect(pr.locator("span[aria-hidden]").last()).toHaveText("#482");
  await pr.click();
  await expect(page).toHaveURL("/pr/acme/node-client/482");
  await expect(page.getByRole("link", { name: "Open review" })).toBeVisible();
});

test("opening a PR becomes a history entry with C17", async ({ page }) => {
  await page.goto("/prs?scenario=empty-pr-history");
  await expect(page.getByText(/No pull requests yet/)).toBeVisible();
  await page.getByLabel("GitHub pull request URL").fill("acme/node-client#482");
  await page.getByRole("button", { name: "Open", exact: true }).click();
  await expect(page.getByRole("heading", { level: 1, name: /^acme\/node-client #482/ })).toBeVisible();
  await page
    .getByRole("navigation", { name: "Breadcrumb" })
    .getByRole("link", { name: "Pull requests" })
    .click();
  const row = page.getByRole("listitem").filter({ hasText: "acme/node-client #482" });
  await expect(row).toContainText("opened");
  await expect(row.getByRole("link", { name: "Review acme/node-client #482", exact: true })).toHaveCount(0);
});
