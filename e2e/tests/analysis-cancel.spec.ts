// Journey (a), part 1: progress while planning, cancel, analyse again; failure before any flow → Retry runs a
// new round to the end with progress k of n.
import { analyze, checkScreen, PR_URL, signIn, waitForState } from "../support/app";
import { expect, test } from "../support/fixtures";

test.describe.configure({ mode: "serial" });

const progress = (page: import("@playwright/test").Page) =>
  page.getByRole("progressbar", { name: "Analysis progress" });

test("cancel while planning: Home says the analysis was cancelled, with the URL filled in", async ({
  page,
  stack,
}) => {
  test.skip(!stack.script, "needs the fake provider's script (a local stack)");
  stack.script?.([{ id: "slow-plan", job: "plan", action: { replay: { step_delay_ms: 1000 } } }]);
  await signIn(page);
  const id = await analyze(page);
  await expect(page.getByText("Planning flows…")).toBeVisible();
  await expect(progress(page)).not.toHaveAttribute("aria-valuenow", /.*/);
  await expect(page.getByRole("link", { name: /^(Resume|Open) review$/ })).toHaveCount(0);
  await checkScreen(page);

  await page.getByRole("button", { name: "Cancel analysis" }).click();
  await expect(page.getByRole("group", { name: "Confirm cancellation" })).toContainText(
    "The agent's session for this pull request is kept. Pause keeps everything.",
  );
  await page.getByRole("button", { name: "Confirm cancel", exact: true }).click();
  await expect(page).toHaveURL(/\/$/);
  await expect(
    page.getByRole("status").filter({ hasText: "Analysis cancelled — analyze again." }),
  ).toBeVisible();
  await expect(page.getByLabel("GitHub pull request URL")).toHaveValue(PR_URL);
  await waitForState(page, id, "cancelled", 20_000);
  await checkScreen(page);
});

test("failure before any flow: plain description; Retry runs a new round to the end, k of n", async ({
  page,
  stack,
}) => {
  test.skip(!stack.script, "needs the fake provider's script (a local stack)");
  stack.script?.([
    { id: "plan-fails", job: "plan", times: 1, action: { fail: { message: "scripted plan failure" } } },
    { id: "slow-flows", job: "analyse_flow", action: { replay: { step_delay_ms: 150 } } },
  ]);
  await signIn(page);
  const id = await analyze(page);
  await waitForState(page, id, "failed");
  await expect(page.getByText("The analysis failed before any flow was planned")).toBeVisible();
  await expect(page.getByRole("link", { name: /^(Resume|Open) review$/ })).toHaveCount(0);
  await checkScreen(page);

  await page.getByRole("button", { name: "Retry" }).click();
  await expect(page.getByText(/^[1-4] of 5 flows analysed/)).toBeVisible({ timeout: 60_000 });
  // The bar counts the whole run in percent (brief §4.2); the flow count k of n is in its text.
  await expect(progress(page)).toHaveAttribute("aria-valuemax", "100");
  await expect(progress(page)).toHaveAttribute(
    "aria-valuetext",
    /^\d+% of analysis · [1-4] of 5 flows analysed/,
  );
  await checkScreen(page);
  await waitForState(page, id, "finished");
  await expect(page.getByText("5 of 5 flows analysed")).toBeVisible();
  await expect(page.getByRole("link", { name: "Open review" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Cancel analysis" })).toHaveCount(0);
  await checkScreen(page);
});
