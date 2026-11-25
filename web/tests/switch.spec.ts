import { expect, test } from "./fixtures";
import { expectAccessible, expectKeyboardTooltips, PR_URL } from "./screens-support";

// Plan §F.1 after the switch: My work at `/`, Pull requests at `/prs`, old links redirected.
test("My work is the start page; the shell leads to Pull requests and back", async ({ page }) => {
  await page.goto("/");
  const nav = page.getByRole("navigation", { name: "Main" });
  await expect(nav.getByRole("link", { name: /^My work/ })).toHaveAttribute("aria-current", "page");
  await expect(page.getByRole("main").getByRole("list").first()).toBeVisible();
  await expectAccessible(page);
  await expectKeyboardTooltips(page);

  await nav.getByRole("link", { name: "Pull requests" }).click();
  await expect(page).toHaveURL(/\/prs$/);
  await expect(page.getByLabel("GitHub pull request URL")).toBeVisible();
  await expect(nav.getByRole("link", { name: "Pull requests" })).toHaveAttribute("aria-current", "page");
  await expectAccessible(page);
  await expectKeyboardTooltips(page);

  await nav.getByRole("link", { name: /^My work/ }).click();
  await expect(page).toHaveURL(/\/$/);
});

test("an old /?pr= link lands on the PR page and checks it on arrival", async ({ page }) => {
  await page.goto(`/?pr=${encodeURIComponent(PR_URL)}`);
  await expect(page).toHaveURL("/pr/acme/node-client/482");
  await expect(page.getByRole("button", { name: "Start analysis", exact: true })).toBeVisible();
});

test("the tab title rule of My work holds at `/`; Pull requests leaves it", async ({ page }) => {
  await page.goto("/?dashboard=refreshing");
  await expect(page).toHaveTitle("diffpilot My work");
  // The running scan is loaded before the clock moves past the 10 s threshold.
  await expect(page.getByRole("progressbar", { name: "Scan progress" })).toBeVisible();
  await page.clock.fastForward(11_000);
  await expect(page).toHaveTitle("diffpilot My work: busy");
  await page.getByRole("navigation", { name: "Main" }).getByRole("link", { name: "Pull requests" }).click();
  await expect(page).not.toHaveTitle(/My work/);
});
