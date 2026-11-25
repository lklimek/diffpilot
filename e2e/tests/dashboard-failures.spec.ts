// A refresh GitHub refuses (v1.1 plan §A.3): its primary rate limit fails the refresh with GitHub's reset time
// and blocks a new start until then; an outage fails it with a plain message. The previous list stays both times.
import { checkScreen, clock, signIn } from "../support/app";
import {
  api,
  githubFaults,
  latestRefresh,
  ranked,
  refreshButton,
  rowIds,
  saveSettings,
  waitForRefresh,
  workList,
  worldSettings,
} from "../support/dashboard";
import { expect, test } from "../support/fixtures";
import { screens } from "../support/screens";

test.describe.configure({ mode: "serial" });

test("a first refresh stores the list the failures keep", async ({ page, stack }) => {
  test.skip(!stack.github, "needs fake GitHub's test controls (a local stack)");
  await signIn(page);
  await saveSettings(page, worldSettings());
  await refreshButton(page).click();
  await waitForRefresh(page, ranked);
  await expect(workList(page).locator("li[data-item]").first()).toBeVisible();
});

test("GitHub's rate limit: the refresh fails with the retry time; Refresh waits for it", async ({
  page,
  stack,
}) => {
  test.skip(!stack.github, "needs fake GitHub's test controls (a local stack)");
  test.setTimeout(300_000); // waits for the reset, up to two minutes ahead
  await signIn(page);
  const before = await rowIds(page);
  const shown = await latestRefresh(page);
  // A whole minute ahead, so the shown HH:MM is the reset's own minute whatever the second.
  const reset = Math.ceil(Date.now() / 60_000) * 60 + 60;
  await githubFaults(page, stack, [
    { on: "search", matching: "", times: 1, answer: { kind: "primary_rate_limit", reset, status: 403 } },
  ]);
  await refreshButton(page).click();
  const failed = await waitForRefresh(page, (r) => r.id !== shown?.id && r.state === "failed", 60_000);
  expect(failed.failure?.code).toBe("github_rate_limited");
  expect(Date.parse(failed.failure?.retry_at ?? "")).toBe(reset * 1000);
  const at = await clock(page, failed.failure?.retry_at ?? "");

  await expect(page.getByRole("button", { name: `Retry at ${at}` })).toHaveCount(2);
  for (const button of await page.getByRole("button", { name: `Retry at ${at}` }).all())
    await expect(button).toHaveAttribute("aria-disabled", "true");
  await expect(page.getByText(new RegExp(`GitHub allows new requests from ${at} \\(in `))).toBeVisible();
  await expect(
    page.getByText(/^Nothing was replaced: you are looking at the \d\d:\d\d snapshot\./),
  ).toBeVisible();
  await expect(page.getByText(/^Previous snapshot · \d\d:\d\d$/)).toBeVisible();
  expect(await rowIds(page)).toEqual(before);
  await checkScreen(page);
  await screens(page, "dashboard-rate-limited");

  const refused = await api(page, "POST", "/api/v1/dashboard/refreshes");
  expect([refused.status, refused.body?.code]).toEqual([429, "github_rate_limited"]);
  // Once the reset has passed, Retry is offered again.
  await expect(page.getByRole("button", { name: "Retry", exact: true })).toBeVisible({ timeout: 150_000 });
});

test("GitHub down: the refresh fails with a plain message and the previous list stays", async ({
  page,
  stack,
}) => {
  test.skip(!stack.github, "needs fake GitHub's test controls (a local stack)");
  await signIn(page);
  await expect(workList(page).locator("li[data-item]").first()).toBeVisible();
  const before = await rowIds(page);
  // More failures than the scan tries (last test of this stack: leftovers do not matter).
  await githubFaults(page, stack, [
    { on: "search", matching: "", times: 100, answer: { kind: "server_error", status: 502, html: true } },
  ]);
  const shown = await latestRefresh(page);
  await page.getByRole("button", { name: "Retry", exact: true }).click();
  const failed = await waitForRefresh(page, (r) => r.id !== shown?.id && r.state === "failed", 90_000);
  expect(failed.failure).toEqual({
    code: "github_unavailable",
    message: "GitHub could not be read. Try again later.",
    retry_at: null,
  });
  // The error banner's title.
  await expect(
    page.getByRole("strong").filter({ hasText: /^GitHub could not be read\. Try again later\.$/ }),
  ).toBeVisible();
  await expect(
    page.getByText(/^Nothing was replaced: you are looking at the \d\d:\d\d snapshot\./),
  ).toBeVisible();
  const snapshot = page.getByText(/^Full scan \d\d:\d\d$/);
  await expect(snapshot).toBeVisible();
  await expect(snapshot).toHaveAccessibleDescription(/Scan started at .* failed\. Last full scan:/);
  expect(await rowIds(page)).toEqual(before);
  await checkScreen(page);
  await screens(page, "dashboard-github-down");
});
