// Journey (a), cancel while a flow job runs: the job stops, the stored flows and the agent session are discarded,
// and a new analysis of the same head starts clean (http-api.md "Cancel and retry").
import { analyze, checkScreen, PR_URL, runnerEvents, signIn, sql, waitForState } from "../support/app";
import { expect, test } from "../support/fixtures";

/** Claim interval of the runner: no new attempt may start within two of them after the cancel. */
const CLAIM_INTERVAL_MS = 5_000;

test("cancel after a stored flow while the next flow runs: job stopped, results and session discarded, clean restart", async ({
  page,
  stack,
}) => {
  test.skip(!stack.script || !stack.stateDir, "needs the fake provider's script and the stack's files");
  stack.script?.([
    {
      id: "slow-flow-2",
      job: "analyse_flow",
      flow: 2,
      times: 1,
      action: { replay: { step_delay_ms: 3_000 } },
    },
  ]);
  await signIn(page);
  const id = await analyze(page);
  const documents = (kind: string) =>
    Number(sql(stack, `SELECT count(*) FROM documents WHERE analysis_id = '${id}' AND kind = '${kind}'`)[0]);
  // Flow 1 stored, the slow flow 2 job running.
  await expect.poll(() => documents("flow"), { timeout: 60_000 }).toBeGreaterThanOrEqual(1);
  await expect
    .poll(
      () => runnerEvents(stack).filter((e) => e.event === "claimed" && e.kind === "analyse_flow").length,
      {
        timeout: 30_000,
      },
    )
    .toBe(2);
  const running = runnerEvents(stack).filter((e) => e.event === "claimed" && e.kind === "analyse_flow")[1];
  expect(runnerEvents(stack).some((e) => e.event === "finished" && e.job === running.job)).toBe(false);
  expect(sql(stack, `SELECT session_state FROM analyses WHERE id = '${id}'`)).toEqual(["active"]);

  await page.getByRole("button", { name: "Cancel analysis" }).click();
  await expect(page).toHaveURL(/\/$/);
  await waitForState(page, id, "cancelled", 20_000);
  const cancelledAt = Date.now();
  await checkScreen(page);

  // The running attempt ends without a result, and nothing is claimed afterwards.
  await expect
    .poll(() => runnerEvents(stack).find((e) => e.event === "finished" && e.job === running.job)?.outcome, {
      timeout: 2 * CLAIM_INTERVAL_MS,
    })
    .toMatch(/^(?!succeeded$)/);
  await page.waitForTimeout(2 * CLAIM_INTERVAL_MS + 1_000);
  expect(
    runnerEvents(stack).filter((e) => e.event === "claimed" && e.at > cancelledAt),
    "no job claimed after the cancel",
  ).toEqual([]);
  expect(documents("flow"), "stored flows discarded").toBe(0);
  expect(sql(stack, `SELECT count(*) FROM documents WHERE analysis_id = '${id}'`)).toEqual(["0"]);
  await expect
    .poll(() => sql(stack, `SELECT session_state FROM analyses WHERE id = '${id}'`)[0], { timeout: 30_000 })
    .toBe("dropped");

  // Analyse again: a new analysis that runs to the end on its own session.
  await expect(page.getByLabel("GitHub pull request URL")).toHaveValue(PR_URL);
  const fresh = await analyze(page);
  expect(fresh).not.toBe(id);
  await waitForState(page, fresh, "finished");
  await expect(page.getByText("5 of 5 flows analysed")).toBeVisible();
  expect(
    Number(sql(stack, `SELECT count(*) FROM documents WHERE analysis_id = '${fresh}' AND kind = 'flow'`)[0]),
  ).toBe(5);
  expect(documents("flow"), "the cancelled analysis stays empty").toBe(0);
  await checkScreen(page);
});
