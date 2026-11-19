// Journey (b), flow groups: a flow the LLM classifies as a refactor comes after the behaviour flows, labelled as an
// LLM classification in the navigation, the flow introduction and the Inspector. Test code is outside the agent's
// scope: no flow is about tests; the tests are listed once, as a deterministic quiet row.
import { readFileSync } from "node:fs";
import { checkScreen, finishedReview, heading, inspector, nav, navTo } from "../support/app";
import { expect, test } from "../support/fixtures";

test("an LLM-classified refactor flow follows the behaviour flows, labelled; tests are listed once, not as a flow", async ({
  page,
  stack,
}) => {
  test.skip(!stack.script, "needs the fake provider's script (a local stack)");
  const plan = JSON.parse(
    readFileSync(new URL("../../fixtures/pr-482/agent/plan.json", import.meta.url), "utf8"),
  ) as { flows: { id: string; classification: string }[] };
  // The second most important flow of the fixture becomes a pure refactor.
  for (const flow of plan.flows) if (flow.id === "f2") flow.classification = "refactor_pure";
  stack.script?.([{ id: "groups", job: "plan", action: { answer: { output: plan } } }]);
  await finishedReview(page);

  const entries = (
    await nav(page)
      .getByRole("button")
      .evaluateAll((buttons) =>
        buttons.map((b) => (b.getAttribute("aria-label") ?? b.textContent ?? "").trim()),
      )
  ).filter((name) => /^Flow \d/.test(name));
  expect(entries.map((name) => name.replace(/^(Flow \d+: [^,]*?)(?: LLM-classified \w+)?$/, "$1"))).toEqual([
    "Flow 1: Send an RPC request with retry",
    "Flow 2: New error variant RpcError::Exhausted",
    "Flow 3: Removed the single-attempt API send_once()",
    "Flow 4: Move of backoff helpers util/backoff.rs → rpc/retry.rs",
    "Flow 5: Default timeout 30 s → 5 s",
  ]);
  const row = (title: string) => nav(page).getByRole("listitem").filter({ hasText: title });
  await expect(row("Default timeout 30 s → 5 s")).toContainText("LLM-classified refactor");
  for (const title of [
    "Send an RPC request with retry",
    "New error variant",
    "Removed the single-attempt API",
    "Move of backoff helpers",
  ]) {
    await expect(row(title)).not.toContainText("LLM-classified");
  }
  // No test flow and no tests label: the tests are one deterministic row of Not analysed.
  await expect(nav(page)).not.toContainText("LLM-classified tests");
  await expect(nav(page).getByRole("button", { name: /^Tests · 4 files/ })).toHaveCount(1);
  await checkScreen(page);

  await navTo(page, /^Flow 5:/);
  await expect(heading(page)).toContainText("Default timeout 30 s → 5 s");
  await expect(page.getByRole("main")).toContainText("LLM-classified refactor");
  await expect(inspector(page)).toContainText("LLM-classified refactor");
  await checkScreen(page);
});
