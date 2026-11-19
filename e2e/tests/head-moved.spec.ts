// Journey (c)/(e): the PR head moves after the analysis — a comment answers pr_head_changed, Home shows the moved
// head with Start analysis, the new analysis starts without ticks, and Ask on the superseded analysis is still answered
// (the PR keeps its session).
import { type Page } from "@playwright/test";
import {
  analyseAllRemainingFlows,
  checkPr,
  checkScreen,
  finishedReview,
  heading,
  inspector,
  nav,
  openStep,
  startAnalysis,
  waitForState,
} from "../support/app";
import { expect, test } from "../support/fixtures";

const pending = async (page: Page, id: string) =>
  (
    (await (await page.request.get(`/api/v1/analyses/${id}/pending-review`)).json()) as {
      comments: unknown[];
    }
  ).comments;

test("head moved: comment refused with pr_head_changed, Start analysis, the superseded analysis still takes Ask", async ({
  page,
  stack,
}) => {
  test.skip(!stack.moveHead, "needs the clone base and fake GitHub's head control (a local stack)");
  stack.script?.([
    {
      id: "old-analysis",
      job: "ask",
      prompt_contains: "still current",
      action: { answer: { output: { answer: "Answered on the superseded analysis.", evidence: [] } } },
    },
  ]);
  const old = await finishedReview(page);
  await openStep(page, 1, 2);
  await page.keyboard.press("x");
  await expect(nav(page).getByRole("button", { name: /^Step 2: .*, reviewed$/ })).toBeVisible();
  const moved = (await stack.moveHead?.()) ?? "";

  await page.getByRole("main").getByRole("button", { name: "Comment on line 96", exact: true }).click();
  await inspector(page)
    .getByRole("textbox", { name: "Comment on src/rpc/client.rs:96" })
    .fill("After the push?");
  await inspector(page).getByRole("button", { name: "Add to GitHub pending review" }).click();
  await expect(inspector(page).getByRole("alert")).toContainText(
    "The pull request changed since this analysis. Analyse it again to comment.",
  );
  // A definite refusal: no Retry that could not succeed, no "may already be in your pending review".
  await expect(inspector(page).getByRole("button", { name: "Retry" })).toHaveCount(0);
  expect(await pending(page, old)).toEqual([]);
  await checkScreen(page);

  await page.goto("/review");
  await checkPr(page);
  const banner = page.getByRole("alert").filter({ hasText: "The PR has moved." });
  await expect(banner).toContainText(`This analysis is for 52d2da1; the PR is now at ${moved.slice(0, 7)}.`);
  await expect(banner.getByRole("link", { name: "Open the old analysis" })).toHaveAttribute(
    "href",
    `/analyses/${old}`,
  );
  await checkScreen(page);
  const fresh = await startAnalysis(page);
  expect(fresh).not.toBe(old);
  await analyseAllRemainingFlows(page);
  await waitForState(page, fresh, "finished");
  await page.getByRole("link", { name: "Open", exact: true }).click();
  await expect(page.getByRole("banner")).toContainText(moved.slice(0, 7));
  await openStep(page, 1, 2);
  await expect(nav(page).getByRole("button", { name: /^Step 2: .*, reviewed$/ })).toHaveCount(0);

  await page.goto(`/analyses/${old}`);
  await expect(heading(page)).toBeVisible();
  await openStep(page, 1, 5);
  await page.keyboard.press("a");
  await inspector(page)
    .getByRole("textbox", { name: /^Ask the LLM about / })
    .fill("Is this still current?");
  await inspector(page).getByRole("button", { name: "Ask", exact: true }).click();
  // The PR keeps its agent session over a fresh analysis: the old analysis still takes questions (refused only when the
  // PR's data was explicitly removed, http-api.md Ask) and the question is answered on it.
  await expect(inspector(page).getByRole("alert")).toHaveCount(0);
  const asked = inspector(page).getByRole("list", { name: "Ask conversation" });
  await expect(asked).toContainText("Asked by you: Is this still current?");
  await expect(asked).toContainText("Answered on the superseded analysis.", { timeout: 60_000 });
  await checkScreen(page);
});
