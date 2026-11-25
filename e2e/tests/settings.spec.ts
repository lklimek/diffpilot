// Journey (f): Settings — LLM status from the most recent LLM call (failed, then working), language of the LLM
// texts, GitHub App installations; signed-out screens; Sign out.
import {
  analyseAllRemainingFlows,
  analyze,
  checkScreen,
  signIn,
  userMenuButton,
  waitForState,
} from "../support/app";
import { signInForDashboardSetup } from "../support/dashboard";
import { expect, test } from "../support/fixtures";

test.describe.configure({ mode: "serial" });

test("signed out: Home and Settings offer sign-in only", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("link", { name: "Sign in with GitHub" })).toBeVisible();
  await checkScreen(page);
  await page.goto("/settings/llm");
  await expect(page.getByRole("link", { name: "Sign in with GitHub" })).toBeVisible();
  await expect(page.getByRole("region", { name: "Usage and limits" })).toHaveCount(0);
  await checkScreen(page);
});

test("after a failed LLM call the status is a plain sentence with its time; Retry makes it working", async ({
  page,
  stack,
}) => {
  test.skip(!stack.script, "needs the fake provider's script (a local stack)");
  stack.script?.([{ id: "plan-fails", job: "plan", times: 1, action: { fail: { message: "scripted" } } }]);
  // Opening My work starts a ranking whose calls would change the last-call outcome.
  await signInForDashboardSetup(page);
  await page.goto("/settings/llm");
  const lastCall = page.getByRole("region", { name: "Last LLM call" });
  await expect(lastCall).toContainText("No LLM call yet.");
  await checkScreen(page);

  await page.goto("/review");
  const id = await analyze(page);
  await waitForState(page, id, "failed");
  await page.goto("/settings/llm");
  await expect(lastCall.getByText(/^Failed · .*\d\d:\d\d/)).toBeVisible();
  await expect(lastCall).not.toContainText(/scripted|error:|\{/);
  const account = userMenuButton(page);
  await expect(account).toHaveAccessibleName("alice — LLM not working");
  await expect(account).toHaveAttribute("aria-expanded", "false");
  await expect(account).toHaveAccessibleDescription("alice — LLM not working");
  await account.click();
  const activity = page.getByRole("region", { name: "LLM activity" });
  await expect(activity.getByText("LLM not working", { exact: true })).toBeVisible();
  await expect(activity.getByText(/^Last LLM call \d\d:\d\d · failed\.$/)).toBeVisible();
  await expect(activity).not.toContainText(/scripted|error:|\{/);
  await checkScreen(page);

  await page.goto(`/analyses/${id}/progress`);
  await page.getByRole("button", { name: "Retry" }).click();
  await analyseAllRemainingFlows(page);
  await waitForState(page, id, "finished");
  await page.goto("/settings/llm");
  await expect(lastCall.getByText(/^Succeeded · .*\d\d:\d\d/)).toBeVisible();
  await expect(account).toHaveAccessibleName("alice: account and LLM activity");
  await checkScreen(page);
  await page.goto("/settings/github-sync");
  await expect(page.getByRole("region", { name: "GitHub App" })).toContainText("acme");
  await checkScreen(page);
  await page.goto("/settings?repo=acme%2Fnode-client");
  await expect(page).toHaveURL(/\/settings\/repositories\?repo=acme%2Fnode-client$/);
  await expect(
    page.getByRole("button", { name: "Hide analysis rules for acme/node-client" }),
  ).toHaveAttribute("aria-expanded", "true");
  await expect(page.getByRole("region", { name: /acme\/node-client/ })).toBeVisible();
  await checkScreen(page);
});

test("the language of the LLM texts is saved", async ({ page }) => {
  await signIn(page);
  await page.goto("/settings/llm");
  const select = page.getByLabel(/written in/);
  await expect(select).toHaveValue("en");
  await select.selectOption("pl");
  await expect(select).toBeEnabled();
  await page.reload();
  await expect(page.getByLabel(/written in/)).toHaveValue("pl");
  await checkScreen(page);
});

test("Sign out ends the session and shows the signed-out Home", async ({ page }) => {
  await signIn(page);
  await page.goto("/settings");
  const account = page.getByRole("region", { name: "GitHub sign-in" });
  await expect(account).toContainText("Signed in as alice");
  await checkScreen(page);
  await userMenuButton(page).click();
  const menu = page.getByRole("dialog", { name: "alice: account and LLM activity" });
  await expect(menu).toContainText("Signed in as alice (GitHub)");
  await menu.getByRole("button", { name: "Sign out", exact: true }).click();
  await expect(page).toHaveURL(/\/$/);
  await expect(page.getByRole("link", { name: "Sign in with GitHub" })).toBeVisible();
  // The session is gone on the server too, not only in this page.
  await page.reload();
  await expect(page.getByRole("link", { name: "Sign in with GitHub" })).toBeVisible();
});

// TODO(installations not checked): fake GitHub always answers the installations list; the "not checked" state
// needs it to fail on request.
