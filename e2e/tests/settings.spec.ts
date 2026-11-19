// Journey (f): Settings — LLM status from the most recent LLM call (failed, then working), language of the LLM
// texts, GitHub App installations; signed-out screens; Sign out.
import { analyseAllRemainingFlows, analyze, checkScreen, signIn, waitForState } from "../support/app";
import { expect, test } from "../support/fixtures";

test.describe.configure({ mode: "serial" });

test("signed out: Home and Settings offer sign-in only", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("link", { name: "Sign in with GitHub" })).toBeVisible();
  await checkScreen(page);
  await page.goto("/settings");
  await expect(page.getByRole("link", { name: "Sign in with GitHub" })).toBeVisible();
  await expect(page.getByRole("region", { name: "LLM status" })).toHaveCount(0);
  await checkScreen(page);
});

test("after a failed LLM call the status is a plain sentence with its time; Retry makes it working", async ({
  page,
  stack,
}) => {
  test.skip(!stack.script, "needs the fake provider's script (a local stack)");
  stack.script?.([{ id: "plan-fails", job: "plan", times: 1, action: { fail: { message: "scripted" } } }]);
  await signIn(page);
  await page.goto("/settings");
  await expect(page.getByRole("region", { name: "LLM status" })).toContainText("not known yet");
  await checkScreen(page);

  await page.goto("/review");
  const id = await analyze(page);
  await waitForState(page, id, "failed");
  await page.goto("/settings");
  const llm = page.getByRole("region", { name: "LLM status" });
  await expect(llm.getByText("not working")).toBeVisible();
  await expect(llm).toContainText("last LLM call:");
  await expect(llm).not.toContainText(/scripted|error:|\{/);
  await checkScreen(page);

  await page.goto(`/analyses/${id}/progress`);
  await page.getByRole("button", { name: "Retry" }).click();
  await analyseAllRemainingFlows(page);
  await waitForState(page, id, "finished");
  await page.goto("/settings");
  await expect(llm.getByText("working", { exact: true })).toBeVisible();
  await expect(page.getByRole("region", { name: "GitHub App" })).toContainText("acme");
  await expect(page.getByRole("region", { name: /acme\/node-client/ })).toBeVisible();
  await checkScreen(page);
});

test("the language of the LLM texts is saved", async ({ page }) => {
  await signIn(page);
  await page.goto("/settings");
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
  const account = page.getByRole("region", { name: "Account" });
  await expect(account).toContainText("Signed in as alice");
  await checkScreen(page);
  await account.getByRole("button", { name: "Sign out", exact: true }).click();
  await expect(page).toHaveURL(/\/$/);
  await expect(page.getByRole("link", { name: "Sign in with GitHub" })).toBeVisible();
  // The session is gone on the server too, not only in this page.
  await page.reload();
  await expect(page.getByRole("link", { name: "Sign in with GitHub" })).toBeVisible();
});

// TODO(installations not checked): fake GitHub always answers the installations list; the "not checked" state
// needs it to fail on request.
