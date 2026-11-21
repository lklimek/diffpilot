// Journey (e): Ask on a step through the job executor and the scripted fake provider — answered and pinned,
// kept over a reload, a wait on the usage limit with its time, a failed answer as a plain sentence; LLM text with
// markup renders as text.
import { type Page } from "@playwright/test";
import {
  checkScreen,
  clock,
  expectRetriedOnlyAfter,
  finishedReview,
  inspector,
  openStep,
  runnerEvents,
} from "../support/app";
import { expect, test } from "../support/fixtures";

test.describe.configure({ mode: "serial" });

const chat = (page: Page) => page.getByRole("dialog", { name: "Ask" });
const answers = (page: Page) => chat(page).getByRole("list", { name: "Your questions, oldest first" });
/** The thread of `question` in the user's one Ask chat (questions of earlier tests stay in it). */
const entry = (page: Page, question: string) =>
  answers(page).getByRole("listitem", { name: `Question: ${question}`, exact: true });
/** The Inspector's read-only copy of the chat's questions about the open step. */
const stepCopy = (page: Page) =>
  inspector(page).getByRole("region", { name: /^Your questions about this step/ });

/** Asks `question` in the floating chat (a opens it on the open step) and returns its thread. */
async function ask(page: Page, question: string) {
  await page.keyboard.press("a");
  const field = chat(page).getByRole("textbox", { name: "Your question" });
  await expect(field).toBeFocused();
  await field.fill(question);
  await chat(page).getByRole("button", { name: "Ask", exact: true }).click();
  return entry(page, question);
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
  const question = "Can broadcast_tx send the same transaction twice?";
  const thread = await ask(page, question);
  await expect(thread).toContainText("Asked by you");
  // Pinned to the step: the thread names it, and the Inspector of the step keeps a copy.
  await expect(thread).toContainText("flow 1 · step 5");
  await expect(stepCopy(page)).toContainText(question);
  // Backtick spans are drawn as code, without the backticks.
  await expect(thread).toContainText(ANSWER.replaceAll("`", ""), { timeout: 60_000 });
  await expect(thread.locator("code", { hasText: /^broadcast_tx$/ })).toBeVisible();
  await expect(thread).toContainText("LLM answer");
  await expect(thread).toContainText("src/rpc/retry.rs");
  await checkScreen(page);
  await page.reload();
  await expect(stepCopy(page)).toContainText(ANSWER.replaceAll("`", ""));
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
  const id = await finishedReview(page);
  await openStep(page, 1, 6);
  const question = "Answer after the usage limit please: is the delay capped?";
  await ask(page, question);
  let resumesAt = "";
  await expect
    .poll(
      async () => {
        const asked = (await (await page.request.get(`/api/v1/analyses/${id}/questions`)).json()) as {
          text: string;
          resumes_at?: string;
        }[];
        resumesAt = asked.find((entry) => entry.text === question)?.resumes_at ?? "";
        return resumesAt;
      },
      { timeout: 60_000 },
    )
    .not.toBe("");
  expect(Date.parse(resumesAt), "the run time lies ahead").toBeGreaterThan(Date.now());
  await expect(
    answers(page).getByText(
      `Waiting for the LLM usage limit to reset; resumes at ${await clock(page, resumesAt)}.`,
    ),
  ).toBeVisible();
  await checkScreen(page);
  await expect(answers(page)).toContainText("Answered once the limit reset.", { timeout: 90_000 });
  await checkScreen(page);
  // The question's job waited for the reset time it was given, then ran once.
  expectRetriedOnlyAfter(stack, Date.parse(resumesAt), "ask");
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
  const question = "Scripted invalid answer: why give up here?";
  await ask(page, question);
  const failed = entry(page, question).getByText(/^No answer: /);
  await expect(failed).toBeVisible({ timeout: 60_000 });
  await expect(failed).not.toContainText(/[{}<>]|Error|error:|panicked|schema/);
  await expect(entry(page, question).getByRole("button", { name: "Ask again" })).toBeVisible();
  await checkScreen(page);
});

test("Ask again after a failed answer runs the question again and shows its answer", async ({
  page,
  stack,
}) => {
  test.skip(!stack.script, "needs the fake provider's script (a local stack)");
  stack.script?.([
    {
      id: "fails-once",
      job: "ask",
      prompt_contains: "second try",
      times: 1,
      action: { answer: { output: { answer: 42, confidence: "high" } } },
    },
    {
      id: "then-answers",
      job: "ask",
      prompt_contains: "second try",
      action: { answer: { output: { answer: "Answered on the second try.", evidence: [] } } },
    },
  ]);
  await finishedReview(page);
  await openStep(page, 2, 1);
  const question = "On the second try: does the timeout apply per attempt?";
  await ask(page, question);
  await expect(entry(page, question).getByText(/^No answer: /)).toBeVisible({ timeout: 60_000 });
  const asked = runnerEvents(stack).filter(
    (event) => event.event === "claimed" && event.kind === "ask",
  ).length;
  await entry(page, question).getByRole("button", { name: "Ask again" }).click();
  // Asked again in the same place, the retry replaces the failed entry.
  await expect(entry(page, question)).toContainText("Answered on the second try.", { timeout: 60_000 });
  await expect(entry(page, question)).toHaveCount(1);
  expect(
    runnerEvents(stack).filter((event) => event.event === "claimed" && event.kind === "ask").length,
    "Ask again ran a new job",
  ).toBe(asked + 1);
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
  await checkScreen(page);
});
