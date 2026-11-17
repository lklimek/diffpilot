// Journey (e): Ask on a step through the job executor and the scripted fake provider — answered and pinned,
// kept over a reload, a wait on the usage limit with its time, a failed answer as a plain sentence; LLM text with
// markup renders as text.
import { type Page } from "@playwright/test";
import { checkScreen, finishedReview, inspector, openStep } from "../support/app";
import { expect, test } from "../support/fixtures";

test.describe.configure({ mode: "serial" });

const answers = (page: Page) =>
  inspector(page).getByRole("list", { name: "Questions asked about this step" });

async function ask(page: Page, question: string): Promise<void> {
  await page.keyboard.press("a");
  const field = inspector(page).getByRole("textbox", { name: /^Ask the LLM about / });
  await expect(field).toBeFocused();
  await field.fill(question);
  await inspector(page).getByRole("button", { name: "Ask", exact: true }).click();
}

const ANSWER =
  "should_retry gets only the attempt number, so can `broadcast_tx` be sent twice when the first reply is late?";

test("a question on a step is answered, pinned to the step and kept over a reload", async ({
  page,
  stack,
}) => {
  test.skip(!stack.script, "needs the fake provider's script (a local stack)");
  stack.script?.([
    {
      id: "answer",
      job: "ask",
      prompt_contains: "same transaction twice",
      action: {
        answer: {
          output: { answer: ANSWER, evidence: [{ file: "src/rpc/retry.rs", side: "head", line: 23 }] },
        },
      },
    },
  ]);
  await finishedReview(page);
  await openStep(page, 1, 5);
  await ask(page, "Can broadcast_tx send the same transaction twice?");
  await expect(answers(page)).toContainText("You asked: Can broadcast_tx send the same transaction twice?");
  await expect(answers(page)).toContainText(ANSWER, { timeout: 60_000 });
  await expect(answers(page)).toContainText("LLM answer");
  await expect(answers(page)).toContainText("src/rpc/retry.rs");
  await checkScreen(page);
  await page.reload();
  await expect(answers(page)).toContainText(ANSWER);
});

test("usage limit: the question waits with the time it runs, then is answered", async ({ page, stack }) => {
  test.skip(!stack.script, "needs the fake provider's script (a local stack)");
  stack.script?.([
    {
      id: "limit",
      job: "ask",
      prompt_contains: "usage limit please",
      times: 1,
      action: { usage_limit: { resets_in_secs: 15 } },
    },
    {
      id: "after-limit",
      job: "ask",
      prompt_contains: "usage limit please",
      action: { answer: { output: { answer: "Answered once the limit reset.", evidence: [] } } },
    },
  ]);
  await finishedReview(page);
  await openStep(page, 1, 6);
  await ask(page, "Answer after the usage limit please: is the delay capped?");
  await expect(
    answers(page).getByText(/^Waiting for the LLM usage limit to reset; the question runs at \d\d:\d\d\.$/),
  ).toBeVisible({ timeout: 60_000 });
  await checkScreen(page);
  await expect(answers(page)).toContainText("Answered once the limit reset.", { timeout: 90_000 });
});

test("a failed answer reads as a plain sentence and offers Ask again", async ({ page, stack }) => {
  test.skip(!stack.script, "needs the fake provider's script (a local stack)");
  stack.script?.([
    {
      id: "invalid",
      job: "ask",
      prompt_contains: "invalid answer",
      action: { answer: { output: { answer: 42, confidence: "high" } } },
    },
  ]);
  await finishedReview(page);
  await openStep(page, 1, 7);
  await ask(page, "Scripted invalid answer: why give up here?");
  const failed = answers(page).getByText(/^No answer: /);
  await expect(failed).toBeVisible({ timeout: 60_000 });
  await expect(failed).not.toContainText(/[{}<>]|Error|error:|panicked|schema/);
  await expect(answers(page).getByRole("button", { name: "Ask again" })).toBeVisible();
  await checkScreen(page);
});

test("security: an LLM answer with markup is never rendered as markup", async ({ page, stack }) => {
  test.skip(!stack.script, "needs the fake provider's script (a local stack)");
  const markup = '<img src=x onerror="window.__xss=1"><script>window.__xss=2</script>';
  stack.script?.([
    {
      id: "markup",
      job: "ask",
      prompt_contains: "markup",
      action: { answer: { output: { answer: `Look at ${markup} here?`, evidence: [] } } },
    },
  ]);
  await finishedReview(page);
  await openStep(page, 1, 8);
  await ask(page, "Does the answer keep markup as text?");
  // The server refuses HTML in answers (the answer rules); either way nothing becomes markup.
  await expect(answers(page)).toContainText(
    /No answer: The LLM's answer did not follow the answer rules, so it is not shown\.|Look at <img/,
    { timeout: 60_000 },
  );
  await expect(page.locator("#root img, #root script")).toHaveCount(0);
  expect(await page.evaluate(() => (window as { __xss?: number }).__xss)).toBeUndefined();
});
