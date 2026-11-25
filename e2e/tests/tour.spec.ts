// Journey (b): the Tour of a finished analysis — PR context facts, flows and steps, quiet rows, classic diff,
// resume where the reviewer left off, columns.
import {
  checkPr,
  checkScreen,
  finishedReview,
  heading,
  inspector,
  nav,
  navTo,
  openStep,
} from "../support/app";
import { expect, test } from "../support/fixtures";

test.describe.configure({ mode: "serial" });

test("PR context: facts are computed from GitHub and the diff, apart from the LLM text", async ({ page }) => {
  await finishedReview(page);
  const facts = page.getByRole("region", { name: "Facts" });
  await expect(facts).toContainText("From GitHub and the diff; no LLM.");
  for (const fact of [
    "Check runs: 3 passed, 1 failed, 1 skipped, 1 neutral. Statuses: 1 pending.",
    "test (integration) (opens on GitHub in a new tab): sync_headers::slow_peer_2000_headers",
    "3 review threads on GitHub",
    "12 test lines changed or removed, 30 added",
    "3 dependencies changed: tokio, tracing, url",
    "Project profile: repository file",
  ])
    await expect(facts).toContainText(fact);
  // Fake MemCan answered the profile lookup completely (has_more false) and holds no profile.
  await expect(facts).not.toContainText("Project profile in MemCan: not checked");
  const banner = page.getByRole("banner");
  await expect(banner).toContainText(
    "Check runs: 3 passed, 1 failed, 1 skipped, 1 neutral. Statuses: 1 pending.",
  );
  await expect(banner).toContainText("Your progress 0 / 5");
  await expect(page.getByRole("main").locator('[data-provenance="llm"]').first()).toBeVisible();
  await expect(inspector(page)).toContainText(
    "Unexplained: 85 changed lines in 2 files — read these yourself",
  );
  await checkScreen(page);
});

test("flows by importance, then quiet rows with Unexplained first; flow introduction and steps", async ({
  page,
}) => {
  await finishedReview(page);
  const items = await nav(page)
    .getByRole("button")
    .evaluateAll((buttons) =>
      buttons.map((b) => (b.getAttribute("aria-label") ?? b.textContent ?? "").trim()),
    );
  // The remark pills, flag markers and flow tick toggles are their own buttons beside their flow or step.
  const pill =
    /^(\d+ LLM remarks? (in flow|on step) \d+|LLM flags (in flow|on step) \d+:|LLM rated step \d+|Go to the first .* (in flow|on step) \d+$|Mark flow \d+ reviewed$)/;
  expect(
    items.filter((name) => /^(\d+ LLM remarks?|LLM flags) in flow \d+/.test(name)).length,
  ).toBeGreaterThan(0);
  const entries = items.filter((name) => !/column$/.test(name) && !pill.test(name));
  expect(entries.slice(0, 6)).toEqual([
    "PR context",
    "Flow 1: Send an RPC request with retry",
    "Flow 2: Default timeout 30 s → 5 s",
    "Flow 3: New error variant RpcError::Exhausted",
    "Flow 4: Removed the single-attempt API send_once()",
    "Flow 5: Move of backoff helpers util/backoff.rs → rpc/retry.rs",
  ]);
  expect(entries[6]).toMatch(/^Unexplained/);
  expect(entries.slice(7).join(" | ")).toMatch(/Tests · 4 files.*Docs & comments/);

  await page.keyboard.press("j");
  await expect(heading(page)).toHaveText("1 Send an RPC request with retry");
  await expect(page.getByRole("main")).toContainText("0 of 10 steps reviewed");
  await checkScreen(page);

  await openStep(page, 1, 5);
  await expect(heading(page)).toHaveText("5 Retry allowed?");
  const code = page.getByRole("region", { name: /^src\/rpc\/client\.rs, line/ });
  await expect(code).toContainText("should_retry");
  // The assessment lives in the Inspector now and stays collapsed.
  await expect(inspector(page).getByRole("button", { name: /Assessment/ })).toHaveAttribute(
    "aria-expanded",
    "false",
  );
  await checkScreen(page);
});

test("quiet rows: Unexplained and not analysed sections, deterministic only", async ({ page }) => {
  await finishedReview(page);
  const main = page.getByRole("main");
  await navTo(page, /^Unexplained/);
  await expect(main).toContainText("src/rpc/metrics.rs");
  // A function moved within one file is a reorder for the agent, not a quiet move; the fixture plan leaves it out.
  await expect(main).toContainText("src/rpc/client.rs");
  await expect(nav(page)).not.toContainText("Moves with identical text");
  await checkScreen(page);

  for (const [row, content] of [
    [/^Tests · 4 files/, "tests/rpc_retry.rs whole file · rule tests/**"],
    [/^Docs & comments · 5 files/, "README.md lines 15–20 · rule *.md"],
    [/^Mechanical & generated · 1 file/, "Cargo.lock"],
  ] as const) {
    await navTo(page, row);
    await expect(main).toContainText("no LLM");
    await expect(main).toContainText(content);
    await checkScreen(page);
  }
});

test("hard case 3: a step without flags has no Flags section; the low assessment stays collapsed", async ({
  page,
}) => {
  await finishedReview(page);
  await openStep(page, 1, 2);
  await expect(inspector(page).getByRole("button", { name: /^Flags/ })).toHaveCount(0);
  await expect(inspector(page).getByRole("button", { name: /Assessment$/ })).toHaveAttribute(
    "aria-expanded",
    "false",
  );
  await checkScreen(page);
});

test("classic diff of a step: d toggles the base → head view of the file", async ({ page }) => {
  await finishedReview(page);
  await openStep(page, 2, 1);
  const main = page.getByRole("main");
  await expect(main.getByRole("region", { name: /^src\/config\.rs, lines 12 to 26/ })).toBeVisible();
  await page.keyboard.press("d");
  const diff = main.getByRole("region", { name: "Classic diff of src/config.rs" });
  await expect(diff).toBeVisible();
  await expect(diff).toContainText("Duration::from_secs(30)");
  await expect(diff).toContainText("Duration::from_secs(5)");
  await checkScreen(page);
  await page.keyboard.press("d");
  await expect(diff).toHaveCount(0);
});

test("resume where left off: the tick and the last viewed block survive a reload and reopening the PR", async ({
  page,
}) => {
  const id = await finishedReview(page);
  await openStep(page, 2, 1);
  await page.keyboard.press("x");
  const ticked = nav(page).getByRole("button", { name: /^Step 1: .*, reviewed$/ });
  await expect(ticked).toBeVisible();
  // Saved on the server before the reload (the save queue runs in the background).
  await expect
    .poll(async () => (await page.request.get(`/api/v1/analyses/${id}/review-state`)).json())
    .toEqual({ ticked_steps: ["f2.1"], last_viewed: { block: "step", step_id: "f2.1" } });
  await page.reload();
  await expect(heading(page)).toContainText("Default timeout");
  await expect(ticked).toBeVisible();

  await page.goto("/prs");
  await checkPr(page);
  const found = page.getByRole("status").filter({ hasText: "Analysis found." });
  await expect(found).toContainText("Your ticks, answers and last viewed block are kept");
  await checkScreen(page);
  await found.getByRole("link", { name: "Open review", exact: true }).click();
  await expect(heading(page)).toContainText("Default timeout");
  await expect(ticked).toBeVisible();
  await checkScreen(page);
});

test("columns: a hidden column stays hidden after a reload", async ({ page }) => {
  await finishedReview(page);
  await page.getByRole("button", { name: "Hide Inspector column" }).click();
  await page.reload();
  const strip = page.getByRole("button", { name: "Show Inspector column" });
  await expect(strip).toBeVisible();
  await checkScreen(page);
  await strip.click();
  await expect(page.getByRole("button", { name: "Hide Inspector column" })).toBeVisible();
});
