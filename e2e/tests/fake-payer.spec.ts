import { analyze, signIn, sql } from "../support/app";
import { expect, test } from "../support/fixtures";

test("the fake runner claims jobs with the signed-in payer's plan credential", async ({ page, stack }) => {
  test.skip(!stack.stateDir, "needs a local fake stack");
  await signIn(page);
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
