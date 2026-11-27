// The LLM ranking of My work (v1.1 plan §B.3, http-api.md "Refresh states"): it waits behind a running review
// analysis; cancelled while it runs, the snapshot stays with provisional scores and Retry ranking finishes it.
import { analyze, checkScreen, signIn, userMenuButton } from "../support/app";
import {
  api,
  JUDGE_DATA,
  latestRefresh,
  notLiveScan,
  ranked,
  refreshButton,
  saveSettings,
  seedTodos,
  waitForRefresh,
  worldSettings,
} from "../support/dashboard";
import { expect, test } from "../support/fixtures";
import { screens } from "../support/screens";

test.describe.configure({ mode: "serial" });

test("a review analysis running first: the ranking waits for it, then ranks", async ({ page, stack }) => {
  test.skip(
    !stack.script || !stack.memcan,
    "needs the fake provider's script and fake MemCan (a local stack)",
  );
  test.setTimeout(240_000);
  stack.secrets.push(...JUDGE_DATA);
  // The plan job holds the LLM credential for about 100 s (10 recorded lines), well past the scan; it is cancelled
  // once the waiting ranking is seen.
  stack.script?.([{ id: "slow-plan", job: "plan", action: { replay: { step_delay_ms: 10_000 } } }]);
  await signIn(page);
  await saveSettings(page, worldSettings());
  await seedTodos(page, stack);
  const analysis = await analyze(page);
  await expect(page.getByText("Planning flows…")).toBeVisible();

  await page.goto("/");
  const previous = await latestRefresh(page);
  await refreshButton(page).click();
  await waitForRefresh(page, (r) => r.id !== previous?.id && r.ranking?.state === "waiting_for_analysis");
  const account = userMenuButton(page);
  await account.click();
  const activityDisclosure = page.getByRole("button", { name: /^LLM activity/ });
  await activityDisclosure.click();
  const activity = page.getByRole("region", { name: "LLM activity" });
  const waiting = activity
    .getByRole("list", { name: /^Queued/ })
    .getByRole("listitem")
    .filter({ hasText: "Ranking" })
    .getByText("Waits for the review analysis to finish", { exact: true });
  await expect(waiting).toBeVisible();
  await expect(page.getByText(notLiveScan)).toBeVisible();
  await expect(page.getByRole("heading", { name: /^Not judged yet/ })).toBeVisible();
  await checkScreen(page);
  await screens(page, "dashboard-waiting-for-analysis");

  // The analysis ends (cancelled here): the ranking takes the credential and finishes.
  const { status } = await api(page, "POST", `/api/v1/analyses/${analysis}/cancel`);
  expect(status).toBe(200);
  const done = await waitForRefresh(page, ranked);
  expect(done.judged).toBe(18);
  if ((await account.getAttribute("aria-expanded")) === "false") await account.click();
  if ((await activityDisclosure.getAttribute("aria-expanded")) === "false") await activityDisclosure.click();
  await expect(activity).toBeVisible();
  await expect(waiting).toHaveCount(0);
  await expect(page.getByRole("heading", { name: /^Not judged yet/ })).toHaveCount(0);
});

test("cancel during the ranking: provisional scores stay; Retry ranking finishes it", async ({
  page,
  stack,
}) => {
  test.skip(
    !stack.script || !stack.memcan,
    "needs the fake provider's script and fake MemCan (a local stack)",
  );
  // Two new TODOs queue a ranking; previously judged work is reused.
  await signIn(page);
  await seedTodos(page, stack, (todo) => ({ ...todo, id: String(todo.id).replace(/^./, "a") }));
  stack.script?.([{ id: "slow-judge", job: "judge_work", action: { replay: { step_delay_ms: 5000 } } }]);
  const shown = await latestRefresh(page);
  await refreshButton(page).click();
  const running = await waitForRefresh(
    page,
    // The snapshot is stored with the ranking at 0 of 0 for a moment, before the ranking counts what it judges.
    (r) =>
      r.id !== shown?.id && r.state === "ranking" && r.ranking?.state === "running" && r.ranking.total > 0,
  );
  expect(running.ranking?.total).toBe(2);
  const account = userMenuButton(page);
  await account.click();
  await page.getByRole("button", { name: /^LLM activity/ }).click();
  await expect(
    page
      .getByRole("region", { name: "LLM activity" })
      .getByRole("list", { name: /^Running now/ })
      .getByRole("listitem")
      .filter({ hasText: "Ranking" }),
  ).toBeVisible();
  await account.press("Escape");

  const { status } = await api(page, "POST", `/api/v1/dashboard/refreshes/${running.id}/cancel`);
  expect(status).toBe(200);
  const cancelled = await waitForRefresh(
    page,
    (r) => r.id === running.id && r.ranking?.state === "cancelled",
  );
  expect(cancelled.state).toBe("finished");
  const notice = page.getByText(/^Ranking cancelled · Not live · scan \d\d:\d\d$/);
  await expect(notice).toBeVisible();
  await expect(notice).toHaveAccessibleDescription(/ranking.*cancelled/i);
  for (const id of ["TODO-afa91c2e", "TODO-a1bd0a44"]) {
    const row = page.locator("li[data-item]", { hasText: id });
    await expect(row.getByText(/^~-?\d+$/)).toBeVisible();
  }
  await expect(page.getByRole("button", { name: "Retry ranking" })).toBeVisible();
  await checkScreen(page);
  await screens(page, "dashboard-ranking-cancelled");

  stack.script?.([]);
  await page.getByRole("button", { name: "Retry ranking" }).click();
  const done = await waitForRefresh(page, (r) => r.id === running.id && ranked(r));
  expect(done.judged).toBe(2);
  await expect(notice).toHaveCount(0);
  await expect(page.getByRole("heading", { name: /^Not judged yet/ })).toHaveCount(0);
  await checkScreen(page);
});
