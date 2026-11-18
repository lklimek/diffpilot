import { expect, test } from "@playwright/test";
import { expectAccessible } from "./screens-support";
import { ANALYSIS_ID } from "./support";

// Owner request F30: Ask is one conversation per analysis, oldest at the top, the field at the bottom.
test("Ask shows the analysis's conversation as a thread with links to the steps asked on", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`/analyses/${ANALYSIS_ID}?scenario=ask-thread#f1.5`);
  const inspector = page.getByRole("complementary", { name: "Inspector" });
  const thread = inspector.getByRole("list", { name: "Ask conversation" });
  const questions = thread.locator("[data-question]");
  await expect(questions).toHaveText([
    "Asked by you: Can the retry send the same transaction twice?",
    "Asked by you: Where is the timeout set?",
    "Asked by you: Is the reply deduplicated?",
    "Asked by you: Why 5 s?",
  ]);
  // The newest is still being answered: the existing progress line.
  await expect(thread.getByRole("listitem").last().getByRole("status")).toContainText(
    "the conversation is prepared first",
  );
  // List lines of an answer stay one per line.
  await expect(thread.getByText("2. The client retries.")).toBeVisible();
  // The field stays below the conversation; the conversation shows its newest entry.
  const field = inspector.getByRole("textbox", { name: /^Ask the LLM about / });
  expect((await field.boundingBox())!.y).toBeGreaterThan((await questions.last().boundingBox())!.y);
  const box = inspector.getByRole("region", { name: "Ask conversation" });
  await expect
    .poll(() => box.evaluate((e) => e.scrollHeight - e.scrollTop - e.clientHeight))
    .toBeLessThanOrEqual(1);

  await inspector.getByRole("button", { name: "This step", exact: true }).click();
  await expect(questions).toHaveText([
    "Asked by you: Can the retry send the same transaction twice?",
    "Asked by you: Is the reply deduplicated?",
  ]);
  await inspector.getByRole("button", { name: "Whole conversation", exact: true }).click();
  await expectAccessible(page);
  await thread
    .getByRole("button", { name: /step 2:/ })
    .first()
    .click();
  await expect(page).toHaveURL(/#f1\.2$/);
  await expect(page.getByRole("main").getByRole("heading", { level: 2, name: /^2 Attempt/ })).toBeVisible();
});
