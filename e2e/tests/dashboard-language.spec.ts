// Ranking texts follow the user's LLM language (judge prompt version 6): with Polish chosen in Settings, every judge
// job's prompt asks for Polish, as the fake provider received it.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import {
  JUDGE_DATA,
  ranked,
  saveSettings,
  seedTodos,
  signInForDashboardSetup,
  waitForRefresh,
  worldSettings,
} from "../support/dashboard";
import { expect, test } from "../support/fixtures";

test("Polish chosen: every ranking job asks for Polish texts", async ({ page, stack }) => {
  test.skip(
    !stack.script || !stack.stateDir || !stack.memcan,
    "needs the fake provider's script (a local stack)",
  );
  stack.secrets.push(...JUDGE_DATA);
  // Answers as usual; the rule only counts the judge prompts that name Polish as the language.
  stack.script?.([
    {
      id: "polish-judge",
      job: "judge_work",
      prompt_contains: "Polish (polski)",
      action: { replay: { step_delay_ms: 0 } },
    },
  ]);
  await signInForDashboardSetup(page);
  await page.goto("/settings");
  const language = page.getByLabel(/written in/);
  await language.selectOption("pl");
  await expect(language).toBeEnabled();
  await page.reload();
  await expect(page.getByLabel(/written in/)).toHaveValue("pl");

  await saveSettings(page, { ...worldSettings(), schedule_enabled: false });
  await seedTodos(page, stack);
  await page.goto("/");
  const done = await waitForRefresh(page, ranked);
  expect(done.trigger).toBe("automatic");
  expect(done.judged).toBe(18);

  const log = readFileSync(join(stack.stateDir ?? "", "logs", "runner.log"), "utf8");
  const claimed = log.match(/job claimed job=\S+ attempt=\d+ kind="judge_work"/g) ?? [];
  const polish = log.match(/fake provider script rule applied job=\S+ rule=polish-judge/g) ?? [];
  expect(claimed.length, "judge jobs ran").toBeGreaterThan(0);
  expect(polish.length, "every judge prompt asks for Polish").toBe(claimed.length);
});
