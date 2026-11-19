import { expect, test } from "./fixtures";
import { expectAccessible, expectKeyboardTooltips, PR_URL } from "./screens-support";

// Plan §F.1 after the switch: My work at `/`, the review start page at `/review`, old links redirected.
test("My work is the start page; the shell leads to Review a PR and back", async ({ page }) => {
  await page.goto("/");
  const nav = page.getByRole("navigation", { name: "Main" });
  await expect(nav.getByRole("link", { name: /^My work/ })).toHaveAttribute("aria-current", "page");
  await expect(page.getByRole("main").getByRole("list").first()).toBeVisible();
  await expectAccessible(page);
  await expectKeyboardTooltips(page);

  await nav.getByRole("link", { name: "Review a PR" }).click();
  await expect(page).toHaveURL(/\/review$/);
  await expect(page.getByLabel("GitHub pull request URL")).toBeVisible();
  await expect(nav.getByRole("link", { name: "Review a PR" })).toHaveAttribute("aria-current", "page");
  await expectAccessible(page);
  await expectKeyboardTooltips(page);

  await nav.getByRole("link", { name: /^My work/ }).click();
  await expect(page).toHaveURL(/\/$/);
});

test("an old /?pr= link lands on /review with the PR checked on arrival", async ({ page }) => {
  await page.goto(`/?pr=${encodeURIComponent(PR_URL)}`);
  await expect(page).toHaveURL(
    new RegExp(`/review\\?pr=${encodeURIComponent(PR_URL).replace(/[.%]/g, "\\$&")}$`),
  );
  await expect(page.getByLabel("GitHub pull request URL")).toHaveValue(PR_URL);
  await expect(page.getByRole("button", { name: "Start analysis", exact: true })).toBeVisible();
});

test("the tab title rule of My work holds at `/`; Review a PR leaves it", async ({ page }) => {
  await page.goto("/?dashboard=refreshing");
  await expect(page).toHaveTitle("diffpilot My work");
  // The running scan is loaded before the clock moves past the 10 s threshold.
  await expect(page.getByRole("progressbar", { name: "Scan progress" })).toBeVisible();
  await page.clock.fastForward(11_000);
  await expect(page).toHaveTitle("diffpilot My work: busy");
  await page.getByRole("navigation", { name: "Main" }).getByRole("link", { name: "Review a PR" }).click();
  await expect(page).not.toHaveTitle(/My work/);
});
