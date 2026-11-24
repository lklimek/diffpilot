import { analyze, signIn, sql } from "../support/app";
import { expect, test } from "../support/fixtures";

test("the Settings route links the payer through the fake gateway before the runner claims jobs", async ({
  page,
  stack,
}) => {
  test.skip(!stack.stateDir, "needs a local fake stack");
  await signIn(page);
  const status = await (await page.request.get("/api/v1/settings/codex")).json();
  expect(status).toMatchObject({ mode: "chatgpt_plan", state: "linked", plan: "fake" });
  expect(JSON.stringify(status)).not.toMatch(/access_token|refresh_token|id_token|api_key/);
  await analyze(page);
  await expect
    .poll(
      () =>
        sql(
          stack,
          `SELECT EXISTS(SELECT 1 FROM jobs j JOIN llm_credentials c ON c.user_id = j.payer_id
           JOIN users u ON u.id = c.user_id WHERE u.login = 'alice' AND c.mode = 'chatgpt_plan'
           AND c.state = 'linked' AND j.attempt > 0)`,
        ),
      { timeout: 15_000 },
    )
    .toEqual(["1"]);
});
