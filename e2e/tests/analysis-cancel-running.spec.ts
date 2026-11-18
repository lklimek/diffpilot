// Journey (a), cancel while a flow job runs: the job stops, the stored flows are discarded,
// and a new analysis of the same head retains the PR sessions (http-api.md "Cancel and retry").
import {
  analyseAllRemainingFlows,
  analyze,
  checkScreen,
  PR_URL,
  runnerEvents,
  signIn,
  sql,
  waitForState,
} from "../support/app";
import { expect, test } from "../support/fixtures";

/** Claim interval of the runner: no new attempt may start within two of them after the cancel. */
const CLAIM_INTERVAL_MS = 5_000;

test("cancel after a stored flow while the next flow runs: job stopped, results discarded, PR session retained", async ({
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
  await analyseAllRemainingFlows(page);
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
  const prSession = (column: string) =>
    sql(
      stack,
      `SELECT p.${column} FROM pull_requests p JOIN analyses a ON a.pull_request_id=p.id WHERE a.id='${id}'`,
    );
  expect(prSession("session_state")).toEqual(["active"]);
  const thread = prSession("agent_thread_id");

  await page.getByRole("button", { name: "Cancel analysis" }).click();
  await expect(page.getByRole("group", { name: "Confirm cancellation" })).toContainText(
    "The agent's session for this pull request is kept. Pause keeps everything.",
  );
  await page.getByRole("button", { name: "Confirm cancel", exact: true }).click();
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
  expect(prSession("session_state")).toEqual(["active"]);
  expect(prSession("agent_thread_id")).toEqual(thread);

  // Analyse again: a new analysis that runs to the end with the retained PR session.
  await expect(page.getByLabel("GitHub pull request URL")).toHaveValue(PR_URL);
  const fresh = await analyze(page);
  expect(fresh).not.toBe(id);
  await analyseAllRemainingFlows(page);
  await waitForState(page, fresh, "finished");
  await expect(page.getByText("5 of 5 flows analysed")).toBeVisible();
  expect(
    Number(sql(stack, `SELECT count(*) FROM documents WHERE analysis_id = '${fresh}' AND kind = 'flow'`)[0]),
  ).toBe(5);
  expect(documents("flow"), "the cancelled analysis stays empty").toBe(0);
  await checkScreen(page);
});
