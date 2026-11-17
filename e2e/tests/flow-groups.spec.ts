// Journey (b), flow groups: flows the LLM classifies as refactor or tests come after the behaviour flows, refactors
// first, each labelled as an LLM classification in the navigation, the flow introduction and the Inspector.
import { readFileSync } from "node:fs";
import { checkScreen, finishedReview, heading, inspector, nav, navTo } from "../support/app";
import { expect, test } from "../support/fixtures";

test("LLM-classified refactor and tests flows follow the behaviour flows, labelled as LLM classifications", async ({
  page,
  stack,
}) => {
  test.skip(!stack.script, "needs the fake provider's script (a local stack)");
  const plan = JSON.parse(
    readFileSync(new URL("../../fixtures/pr-482/agent/plan.json", import.meta.url), "utf8"),
  ) as { flows: { id: string; classification: string }[] };
  // The two most important flows of the fixture become a test flow and a pure refactor.
  for (const flow of plan.flows) {
    if (flow.id === "f1") flow.classification = "test";
    if (flow.id === "f2") flow.classification = "refactor_pure";
  }
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
    "Flow 1: New error variant RpcError::Exhausted",
    "Flow 2: Removed the single-attempt API send_once()",
    "Flow 3: Move of backoff helpers util/backoff.rs → rpc/retry.rs",
    "Flow 4: Default timeout 30 s → 5 s",
    "Flow 5: Send an RPC request with retry",
  ]);
  const row = (title: string) => nav(page).getByRole("listitem").filter({ hasText: title });
  await expect(row("Default timeout 30 s → 5 s")).toContainText("LLM-classified refactor");
  await expect(row("Send an RPC request with retry")).toContainText("LLM-classified tests");
  for (const title of ["New error variant", "Removed the single-attempt API", "Move of backoff helpers"]) {
    await expect(row(title)).not.toContainText("LLM-classified");
  }
  await checkScreen(page);

  await navTo(page, /^Flow 4:/);
  await expect(heading(page)).toContainText("Default timeout 30 s → 5 s");
  await expect(page.getByRole("main")).toContainText("LLM-classified refactor");
  await expect(inspector(page)).toContainText("LLM-classified refactor");
  await checkScreen(page);
  await navTo(page, /^Flow 5:/);
  await expect(heading(page)).toContainText("Send an RPC request with retry");
  await expect(page.getByRole("main")).toContainText("LLM-classified tests");
  await checkScreen(page);
});
