// My work (v1.1 plan §A.4, §B.3, §F): sign in → the dashboard at `/` → Refresh → scan progress → the provisional
// list → the ranked list (fake provider's neutral judgements), in the tab title too; TODO rows off and on; a row's
// Review link and the moved `/?pr=` deep link; cancel during a scan; MemCan down keeps the TODO rows.
import { checkScreen, memcan, PR_URL, signIn } from "../support/app";
import {
  groupIds,
  JUDGE_DATA,
  latestRefresh,
  ranked,
  refreshButton,
  rowIds,
  saveSettings,
  seedTodos,
  waitForRefresh,
  workList,
  worldSettings,
} from "../support/dashboard";
import { expect, test } from "../support/fixtures";
import { screens } from "../support/screens";

test.describe.configure({ mode: "serial" });

const TODOS = ["TODO-3fa91c2e", "TODO-91bd0a44"];

test("sign in → My work → Refresh → progress → provisional list → ranked list", async ({ page, stack }) => {
  test.skip(
    !stack.script || !stack.memcan,
    "needs the fake provider's script and fake MemCan (a local stack)",
  );
  stack.secrets.push(...JUDGE_DATA);
  // A slow judge keeps the provisional list on screen for a while (4 recorded lines per job of 8 items).
  stack.script?.([{ id: "slow-judge", job: "judge_work", action: { replay: { step_delay_ms: 3000 } } }]);
  await signIn(page);
  await expect(page).toHaveURL(/\/$/);
  await expect(page).toHaveTitle("diffpilot My work");
  await expect(page.getByText("Nothing scanned yet.")).toBeVisible();
  await checkScreen(page);
  await screens(page, "dashboard-empty");

  await saveSettings(page, worldSettings());
  await seedTodos(page, stack);
  await refreshButton(page).click();
  const progress = page.getByRole("region", { name: "Refreshing from GitHub and MemCan" });
  await expect(progress.getByRole("progressbar", { name: "Scan progress" })).toBeVisible();
  await expect(progress.locator("li[data-step]")).toHaveText([
    /Search PRs/,
    /Issues/,
    /PR details/,
    /Ownership & milestones/,
    /History/,
    /MemCan TODOs/,
    /LLM ranking/,
  ]);
  await expect(progress.locator('li[data-step="search_prs"]')).toHaveAttribute("data-state", /running|done/);
  await expect(page.getByRole("button", { name: "Refreshing…" })).toHaveAttribute("aria-disabled", "true");
  await checkScreen(page);
  await screens(page, "dashboard-scanning");
  // Work that lasts 10 s shows in the tab title.
  await expect(page).toHaveTitle("diffpilot My work: busy", { timeout: 30_000 });

  // The scan is stored: every item is provisional ("Not judged yet") until the first judged batch lands.
  // 12 pull requests (the ownership policy keeps PLAT-1203 and PLAT-1199, directed at alice), 6 issues, 2 TODOs;
  // the list opens with every kind shown.
  const banner = page.getByText(/^Ranking with the LLM: \d+ of 20 items judged\.$/);
  await expect(workList(page).getByRole("heading", { name: /^Not judged yet.*·\s*20$/ })).toBeVisible({
    timeout: 90_000,
  });
  expect((await groupIds(page, "Not judged yet")).slice(0, 3)).toEqual(["PLAT-1203", "WUI-320", "NC-482"]);
  await expect(banner).toBeVisible({ timeout: 30_000 });
  await expect(progress).toHaveCount(0);
  await checkScreen(page);
  await screens(page, "dashboard-provisional");

  const done = await waitForRefresh(page, ranked, 150_000);
  expect([done.judged, done.reused]).toEqual([20, 0]);
  await expect(banner).toHaveCount(0);
  await expect(page).toHaveTitle("diffpilot My work: ready");
  await expect(page.getByRole("heading", { name: /^Not judged yet/ })).toHaveCount(0);
  expect(await groupIds(page, "P1")).toEqual(["PLAT-1203", "WUI-320", "NC-482"]);
  expect((await groupIds(page, "P2"))[0]).toBe("WUI-311");
  await expect(page.getByText(/^Refreshed manually at \d\d:\d\d · .* · judged 20 · reused 0$/)).toBeVisible();
  await checkScreen(page);
  await screens(page, "dashboard-ranked");
});

test("TODO rows: hidden when MemCan TODOs are switched off, back when on", async ({ page, stack }) => {
  test.skip(!stack.memcan, "needs fake MemCan");
  await signIn(page);
  await expect(workList(page)).toBeVisible();
  expect(await rowIds(page)).toEqual(expect.arrayContaining(TODOS));
  const off = { ...worldSettings(), include_todos: false };
  await saveSettings(page, off);
  await page.reload();
  await expect(workList(page).locator("li[data-item]").first()).toBeVisible();
  expect((await rowIds(page)).filter((id) => id.startsWith("TODO-"))).toEqual([]);
  const items = await (await page.request.get("/api/v1/dashboard/items")).json();
  expect((items as { items: { kind: string }[] }).items.filter((i) => i.kind === "todo")).toEqual([]);
  await checkScreen(page);
  await saveSettings(page, worldSettings());
  await page.reload();
  await expect(workList(page).getByRole("button", { name: `Details of ${TODOS[0]}` })).toBeVisible();
});

test("a PR row's Review opens /review with the PR checked; /?pr= moves there", async ({ page }) => {
  await signIn(page);
  const row = workList(page).locator('li[data-item="acme~node-client~pr~482"]');
  await row.getByRole("link", { name: /^Review pull request /, exact: false }).click();
  await expect(page).toHaveURL(`/review?pr=${encodeURIComponent(PR_URL)}`);
  await expect(page.getByLabel("GitHub pull request URL")).toHaveValue(PR_URL);
  await expect(page.getByRole("region", { name: "Pull request check" })).toBeVisible();
  await checkScreen(page);

  await page.goto(`/?pr=${encodeURIComponent(PR_URL)}`);
  await expect(page).toHaveURL(`/review?pr=${encodeURIComponent(PR_URL)}`);
  await expect(page.getByRole("region", { name: "Pull request check" })).toBeVisible();
});

test("cancel during a scan: the scan stops, the list stays", async ({ page }) => {
  await signIn(page);
  const before = await rowIds(page);
  const shown = await latestRefresh(page);
  await refreshButton(page).click();
  const progress = page.getByRole("region", { name: "Refreshing from GitHub and MemCan" });
  await expect(progress).toBeVisible();
  await progress.getByRole("button", { name: "Cancel scan" }).click();
  const cancelled = await waitForRefresh(page, (r) => r.id !== shown?.id && r.state === "cancelled", 30_000);
  expect(cancelled.scanned_at).toBeNull();
  await expect(progress).toHaveCount(0);
  await expect(page.getByText(/^Refresh of \d\d:\d\d cancelled · showing \d\d:\d\d$/)).toBeVisible();
  expect(await rowIds(page)).toEqual(before);
  await expect(refreshButton(page)).not.toHaveAttribute("aria-disabled", "true");
  await checkScreen(page);
  await screens(page, "dashboard-scan-cancelled");
});

test("MemCan down: the refresh finishes, TODO rows are kept and a note says they were not refreshed", async ({
  page,
  stack,
}) => {
  test.skip(!stack.memcan, "needs fake MemCan");
  await signIn(page);
  await memcan(page, stack).mode({ mode: "unavailable" });
  try {
    const shown = await latestRefresh(page);
    await refreshButton(page).click();
    const done = await waitForRefresh(page, (r) => r.id !== shown?.id && ranked(r));
    expect(done.failure).toBeNull();
    await expect(page.getByText(/^Refreshed manually at /)).toBeVisible();
    expect(await rowIds(page)).toEqual(expect.arrayContaining(TODOS));
    await page.getByRole("button", { name: /^« More · waiting on others/ }).click();
    await expect(
      page.getByText(
        /^MemCan TODOs not refreshed \(MemCan unreachable at \d\d:\d\d\); the TODO rows are from an earlier scan\.$/,
      ),
    ).toBeVisible();
    await checkScreen(page);
    await screens(page, "dashboard-memcan-down");
  } finally {
    await memcan(page, stack).mode({ mode: "normal" });
  }
});

// Plan §G "Failures": TODO rows kept from an earlier scan say "not refreshed (MemCan unreachable at HH:MM)".
test("MemCan down: each kept TODO row says it was not refreshed", async ({ page, stack }) => {
  await signIn(page);
  await memcan(page, stack).mode({ mode: "unavailable" });
  try {
    const shown = await latestRefresh(page);
    await refreshButton(page).click();
    await waitForRefresh(page, (r) => r.id !== shown?.id && ranked(r));
    for (const id of TODOS) {
      await expect(workList(page).locator("li[data-item]", { hasText: id })).toContainText(
        /not refreshed \(MemCan unreachable at \d\d:\d\d\)/,
      );
    }
  } finally {
    await memcan(page, stack).mode({ mode: "normal" });
  }
});
