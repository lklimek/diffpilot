import type { Page } from "@playwright/test";
import { expect, test } from "./fixtures";
import { expectAccessible, expectTooltips } from "./screens-support";
import { expectTrustWording } from "./support";

// Review loop (docs/review-workflow-plan.md §3, §9.2 named Playwright tests), msw mode.
const PR = "/pr/acme/node-client/482";
const chat = (page: Page) => page.getByRole("dialog", { name: "Ask" });

test("the owner can confirm GitHub proposals from a member and a linked login", async ({ page }) => {
  await page.goto(`${PR}/comments?scenario=comments-proposals&item=C22`);
  await expect(page.getByText(/@alice proposes: Reject/)).toBeVisible();
  await expect(page.getByRole("link", { name: /Proposal on GitHub/ })).toHaveAttribute(
    "href",
    /#discussion_r1041$/,
  );
  await expect(page.getByRole("radio", { name: "Reject", exact: true })).toBeChecked();
  await page
    .getByRole("navigation", { name: "Queue" })
    .getByRole("button", { name: /^C14\b/ })
    .click();
  await expect(page.getByText(/@alice-relay proposes: Resolve/)).toBeVisible();
  await expect(page.getByRole("link", { name: /Proposal on GitHub/ })).toHaveAttribute(
    "href",
    /#discussion_r1042$/,
  );
  await expect(page.getByRole("radio", { name: "Resolve (already fixed)" })).toBeChecked();
  await page.getByRole("button", { name: "Decide", exact: true }).click();
  await page.getByRole("link", { name: "Decision log and agents" }).click();
  await expect(page.getByText(/C14 · Resolve/)).toBeVisible();
});

test("the group and bulk confirmation use the visible GitHub proposals", async ({ page }) => {
  await page.goto(`${PR}/comments?scenario=comments-proposals&item=C18`);
  await expect(page.getByRole("heading", { name: /Same recommendation: Reject/ })).toBeVisible();
  await expect(page.getByText(/@alice-relay proposes: Resolve/)).toBeVisible();
  await page.getByRole("button", { name: "Decide 2 items" }).click();
  await expect(page.getByText(/2 items: decision recorded/)).toBeVisible();
  await page.getByRole("button", { name: /^Decide the rest \(/ }).click();
  await expect(page.getByText(/@alice proposes: Reject/)).toBeVisible();
  const apply = page.getByRole("button", { name: /^Apply \d+ decisions$/ });
  await expect(apply).toHaveAttribute("aria-disabled", "true");
  await page.getByRole("textbox", { name: /^C22 · Reason/ }).fill("Owner's reason");
  await apply.click();
  await page.getByRole("link", { name: "Decision log and agents" }).click();
  await expect(page.getByText(/C18 · Resolve/)).toBeVisible();
  await expect(page.getByText(/C19 · Reject/)).toBeVisible();
  await expect(page.getByText(/C22 · Reject/)).toBeVisible();
});

test("table_row_opens_question", async ({ page }) => {
  await page.goto(`${PR}/comments/table`);
  await page.getByRole("link", { name: "Jitter uses a fixed seed" }).click();
  await expect(page).toHaveURL(`${PR}/comments?item=C22`);
  await expect(page.getByRole("heading", { level: 2, name: /C22 · Jitter uses a fixed seed/ })).toBeFocused();
  await page
    .getByRole("textbox", { name: "Your comment (optional)" })
    .fill("seeded RNG from the test helper");
  await page.getByRole("button", { name: "Decide", exact: true }).click();
  await page.getByRole("navigation", { name: "Comments pages" }).getByRole("link", { name: "Table" }).click();
  const row = page.locator('tr[data-row="C22"]');
  await expect(row).toContainText("Fix here");
  await expect(row).toContainText("in Questions");
  await expect(row).toContainText("seeded RNG from the test helper");
});

test("decide_from_the_middle_opens_the_next_question", async ({ page }) => {
  await page.goto(`${PR}/comments`);
  const queue = page.getByRole("navigation", { name: "Queue" });
  await expect(queue.getByRole("listitem").last()).toContainText("P24");
  await queue.getByRole("button", { name: /^C15(?!\d)/ }).click();
  await expect(page.getByRole("heading", { level: 2, name: /C15 · Debug log prints/ })).toBeFocused();
  const row = queue.getByRole("button", { name: /^C15(?!\d)/ });
  await expect(row.locator("[data-gate='G-SECRET']")).toContainText("Gate G-SECRET");
  const recommendation = row.locator("[data-recommendation='fix_here']");
  await expect(recommendation).toContainText("recommends Fix here");
  await expect(recommendation.locator("[data-provenance='llm']")).toHaveText("LLM");
  const decide = page.getByRole("button", { name: "Decide", exact: true });
  await expect(decide).not.toHaveAttribute("aria-disabled", "true");
  await decide.click();
  await expect(
    page.getByRole("heading", { level: 2, name: /C11 · Backoff cap is not configurable/ }),
  ).toBeFocused();
});

test("Skipped questions can be opened by keyboard without deciding the current question", async ({
  page,
}) => {
  await page.goto(`${PR}/comments?item=C15`);
  await page.getByRole("button", { name: /Skip for now/ }).click();
  const next = page.getByRole("heading", { name: /C11 · Backoff cap is not configurable/ });
  await expect(next).toBeVisible();
  const queue = page.getByRole("navigation", { name: "Queue" });
  await expect(queue.getByRole("button", { name: /^Status: Needs you · 10/ })).toBeVisible();
  const skipped = queue.getByRole("group", { name: "Skipped" });
  await skipped.getByText("Skipped (1)").press("Enter");
  await expect(next).toBeVisible();
  await skipped.getByRole("button", { name: /^C15(?!\d)/ }).press("Enter");
  await expect(page.getByRole("heading", { name: /C15 · Debug log prints/ })).toBeVisible();
  await expect(page.getByRole("button", { name: "Decide", exact: true })).toBeVisible();
});

test("PR Hygiene actions disclose by keyboard without showing diagnostics", async ({ page }) => {
  await page.goto(`${PR}/comments`);
  await page.getByRole("button", { name: "Questions waiting" }).click();
  const status = page.getByRole("dialog", { name: "Pull request status" });
  const preview = status.locator("summary").filter({ hasText: "Waiting on author" });
  const action = status.getByText("Resolve the open review threads");
  await expect(preview).toBeVisible();
  await expect(action).toBeHidden();
  await preview.press("Enter");
  await expect(action).toBeVisible();
  await expect(status.getByRole("link", { name: "Source for PR Hygiene (new tab)" })).toBeVisible();
  for (const colorScheme of ["light", "dark"] as const) {
    await page.emulateMedia({ colorScheme });
    await expectAccessible(page);
  }
});

test("sections_collapse_by_keyboard", async ({ page }) => {
  await page.goto(`${PR}/comments?item=C14`);
  const section = page.getByRole("button", { name: "The discussed code" });
  await expect(section).toHaveAttribute("aria-expanded", "true");
  await section.focus();
  await page.keyboard.press("Enter");
  await expect(section).toHaveAttribute("aria-expanded", "false");
  await expect(page.getByLabel(/Code of src\/rpc\/client.rs/)).toBeHidden();
  await page.reload();
  await expect(page.getByRole("button", { name: "The discussed code" })).toHaveAttribute(
    "aria-expanded",
    "false",
  );
  await page.getByRole("button", { name: "The discussed code" }).focus();
  await page.keyboard.press("Space");
  await expect(page.getByRole("button", { name: "The discussed code" })).toHaveAttribute(
    "aria-expanded",
    "true",
  );
});

test("ask_about_comment_opens_shared_chat", async ({ page }) => {
  await page.goto(`${PR}/comments?item=C14`);
  await expect(page.getByRole("heading", { level: 2, name: /C14 ·/ })).toBeFocused();
  await page.keyboard.press("a");
  await expect(chat(page)).toBeVisible();
  await expect(chat(page)).toContainText("Comment C14");
  await chat(page).getByRole("textbox", { name: "Your question" }).fill("Why is this introduced here?");
  await chat(page).getByRole("textbox", { name: "Your question" }).press("Enter");
  await expect(chat(page).getByText("Why is this introduced here?")).toBeVisible();
  await expect(page.getByRole("button", { name: "1 Ask answer" })).toBeVisible({ timeout: 10_000 });
  await expectTrustWording(page);
});

test("notes_icons_open_new_tab", async ({ page, context }) => {
  await page.goto(`${PR}/comments?item=C14`);
  const notes = page.getByRole("list", { name: "Notes about this code elsewhere" }).getByRole("link");
  await expect(notes.first()).toBeVisible();
  for (const note of await notes.all()) {
    await expect(note).toHaveAttribute("target", "_blank");
    await expect(note).toHaveAccessibleName(/\(new tab\)$/);
  }
  const opened = context.waitForEvent("page");
  await page.getByRole("link", { name: /^Flows: the LLM flagged these lines/ }).click();
  const tab = await opened;
  await expect(tab).toHaveURL(/\/analyses\/[^/]+#f1\.2$/);
});

/** The prompt the help offers for the fixture PR on this diffpilot. */
const agentPrompt = (origin: string) =>
  `Use the diffpilot-decisions skill to carry out my decisions on acme/node-client#482 from diffpilot at ${origin}: ` +
  "subscribe, claim each decision, carry out its effects in this repository's checkout of the PR branch, and " +
  "report each effect done or failed with the commit or comment link.";

test("start_an_agent_from_the_empty_queue_and_the_decision_log", async ({ page, context }) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.goto(`${PR}/comments?scenario=comments-no-agent`);
  const origin = new URL(page.url()).origin;
  const empty = page.getByRole("region", { name: "No undecided questions" });
  const help = empty.getByRole("region", { name: "How to start an agent" });
  await expect(help).toContainText("review_loop_execute");
  await expect(help.locator("[data-agent-prompt]")).toHaveText(agentPrompt(origin));
  await help.getByRole("button", { name: "Copy prompt" }).click();
  await expect(help.getByRole("status")).toHaveText("Copied the prompt.");
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(agentPrompt(origin));
  await expectTooltips(page);
  await expectTrustWording(page);
  for (const colorScheme of ["light", "dark"] as const) {
    await page.emulateMedia({ colorScheme });
    await expectAccessible(page);
  }
  // The Decision log: open while no agent is subscribed, a closed disclosure once one is.
  await page.goto(`${PR}/comments/log?scenario=comments-no-agent`);
  const feed = page.getByRole("region", { name: "Decision feed" });
  await expect(feed.getByRole("region", { name: "How to start an agent" })).toBeVisible();
  await page.goto(`${PR}/comments/log`);
  await expect(feed.getByText("build-host-2")).toBeVisible();
  await expect(feed.getByRole("region", { name: "How to start an agent" })).toHaveCount(0);
  await expect(feed.locator("[data-agent-prompt]")).toBeHidden();
  await feed.getByText("How to start an agent").click();
  await expect(feed.locator("[data-agent-prompt]")).toHaveText(agentPrompt(origin));
  for (const colorScheme of ["light", "dark"] as const) {
    await page.emulateMedia({ colorScheme });
    await expectAccessible(page);
  }
});

for (const [name, url] of [
  ["PR page", PR],
  ["question", `${PR}/comments?item=C14`],
  ["group", `${PR}/comments?item=C18`],
  ["table", `${PR}/comments/table`],
  ["log", `${PR}/comments/log`],
  ["carry-overs", `${PR}/comments/carry-overs`],
  ["watched", "/watching"],
  ["no analysis", `${PR}/comments?item=C11&scenario=comments-no-analysis`],
  ["GitHub error", `${PR}/comments?scenario=comments-github-error`],
] as const) {
  test(`review loop ${name}: tooltips, wording, accessibility in both themes`, async ({ page }) => {
    await page.goto(url);
    await expect(page.getByRole("main")).not.toContainText(/Loading|Reading the comments/);
    await expectTooltips(page);
    await expectTrustWording(page);
    await expectAccessible(page);
    await page.emulateMedia({ colorScheme: "dark" });
    await expectAccessible(page);
  });
}

test("v1.4 queue filters, duplicate decisions, and GitHub provenance", async ({ page }) => {
  await page.goto(`${PR}/comments?scenario=comments-v14&item=C14`);
  const queue = page.getByRole("navigation", { name: "Queue" });
  const status = queue.getByRole("button", { name: /^Status:/ });
  await expect(status).toHaveText(/Needs you · 11.*2 gates/);
  await expect(page.getByRole("link", { name: "Questions, 11 need you" })).toBeVisible();
  await expect(page.getByRole("heading", { name: /C14 · Retry may resend.*LLM/ })).toBeVisible();
  await expect(queue.getByRole("button", { name: /^C30/ })).toHaveCount(0);
  await queue.getByText(/\+2 same point/).click();
  await expect(queue.getByRole("button", { name: /^C30/ })).toBeVisible();
  await expect(
    page.getByRole("link", { name: /C30 \(coderabbit/ }).locator('[data-icon="github"]'),
  ).toBeVisible();
  await page.getByRole("button", { name: "Decide", exact: true }).click();
  await expect(status).toHaveText(/Needs you · 10.*1 gate/);

  await status.press("ArrowDown");
  const menu = page.getByRole("menu");
  await expect(menu.getByRole("menuitemradio")).toHaveCount(9);
  await menu.getByRole("menuitemradio", { name: "Duplicates · 2" }).click();
  await expect(queue.getByRole("button", { name: /^C30/ })).toBeVisible();
  await status.click();
  await menu.getByRole("menuitemradio", { name: /^Decided · / }).click();
  await expect(page).toHaveURL(/status=decided/);
  const imported = queue.getByRole("button", { name: /^C33/ });
  await expect(imported).toContainText("Rejected on GitHub by @alice");
  await expect(queue.getByRole("link", { name: /Evidence for C33/ })).toHaveAttribute(
    "href",
    "https://github.com/acme/node-client/pull/482#discussion_r1038",
  );
  await expect(queue.getByRole("button", { name: /^C9 / })).toContainText("no reason given");
  await page.getByRole("link", { name: "Decision log and agents" }).click();
  await expect(page.getByText(/Rejected on GitHub by @alice/)).toHaveCount(0);
  await page.getByRole("combobox", { name: "Decision source" }).selectOption("github");
  await expect(page.getByText(/Rejected on GitHub by @alice/)).toBeVisible();
});

test("v1.4 assessment dismissal, status and Watch work on a dark phone", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.emulateMedia({ colorScheme: "dark" });
  await page.goto(`${PR}/comments?scenario=comments-v14`);
  const header = page.getByRole("heading", { level: 1 });
  await expect(header).toContainText("acme/node-client #482");
  const watch = page.getByRole("button", { name: "Watch this PR" });
  await expect(watch).toHaveAttribute("aria-pressed", "false");
  await watch.click();
  await expect(page.getByRole("button", { name: "Watching" })).toHaveAttribute("aria-pressed", "true");

  const prStatus = page.getByRole("button", { name: "Questions waiting" });
  await prStatus.press("Enter");
  const panel = page.getByRole("dialog", { name: "Pull request status" });
  await expect(panel.locator("summary").filter({ hasText: /PR Hygiene/ })).toBeVisible();
  await expect(panel.getByRole("heading", { name: /thepastaclaw status/ })).toBeVisible();
  await expect(panel.getByText("CI: 2 passed, 1 pending")).toBeVisible();
  await expect(panel.getByText(/Bot rounds: 4/)).toBeVisible();
  await expect(panel.getByRole("button", { name: "Read GitHub again" })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(prStatus).toBeFocused();

  const queue = page.getByRole("navigation", { name: "Queue" });
  const status = queue.getByRole("button", { name: /^Status:/ });
  const hide = page.getByRole("button", { name: "Hide", exact: true });
  await expect(page.getByText(/Assessing 1 comment now.*1 waits.*1 not assessed: daily limit/)).toBeVisible();
  await hide.press("Space");
  await expect(status).toBeFocused();
  await page.reload();
  await expect(status).toBeVisible();
  await expect(hide).toHaveCount(0);

  await status.click();
  await page.getByRole("menuitemradio", { name: "Waiting for assessment · 1" }).click();
  await expect(queue.getByRole("button", { name: /^C35/ })).toContainText("waiting for assessment");
  await expect(queue.getByRole("button", { name: /^C35/ })).not.toContainText("running");
  await page.reload();
  await expect(status).toHaveText("Waiting for assessment · 1 ▾");
  await expectAccessible(page);
});

test("v1.4 dashboard, PR panel and queue agree on Needs you", async ({ page }) => {
  await page.goto("/?scenario=comments-v14");
  const tally = page.getByRole("link", { name: /11 need you: comments of #482/ });
  await expect(tally).toBeVisible();
  await tally.click();
  await expect(page.getByRole("link", { name: "Questions, 11 need you" })).toBeVisible();
  await expect(page.getByRole("button", { name: /^Status: Needs you · 11/ })).toBeVisible();
  await page.goto(`${PR}?scenario=comments-v14`);
  const panel = page.getByRole("region", { name: "Process PR comments" });
  await expect(panel.getByText("Needs you").locator("xpath=following-sibling::dd[1]")).toContainText("11");
  await expect(panel.getByRole("button", { name: "Watch" })).toBeVisible();
});

test("v1.4 hiding assessment keeps the chosen table decision", async ({ page }) => {
  await page.goto(`${PR}/comments/table?scenario=comments-v14`);
  const decision = page.getByRole("combobox", { name: /^Decision for C22/ });
  await decision.selectOption("resolve");
  await page.getByRole("button", { name: "Hide", exact: true }).click();
  await expect(page).toHaveURL(/\/comments\/table\?/);
  await expect(decision).toHaveValue("resolve");
  await expect(page.getByRole("link", { name: "Table", exact: true })).toBeFocused();
});

test("queue filter count and Undo restores Needs you", async ({ page }) => {
  await page.goto(`${PR}/comments?scenario=comments-v14&item=C22`);
  const queue = page.getByRole("navigation", { name: "Queue" });
  const status = queue.getByRole("button", { name: /^Status:/ });
  await expect(status).toHaveText(/Needs you · 11/);
  await page.getByRole("radio", { name: "Not a review point" }).check();
  const decide = page.getByRole("button", { name: "Decide", exact: true });
  await expect(decide).not.toHaveAttribute("aria-disabled", "true");
  await decide.click();
  await expect(status).toHaveText(/Needs you · 10/);
  await status.click();
  await page.getByRole("menuitemradio", { name: "Not a review point (1)" }).click();
  await expect(queue.getByRole("button", { name: /^C22/ })).toContainText("Not a review point");
  await page.getByRole("link", { name: "Decision log and agents" }).click();
  const entry = page.getByRole("listitem").filter({ hasText: /C22 · Not a review point/ });
  await expect(entry).toContainText("by alice");
  await expectAccessible(page);
  await entry.getByRole("button", { name: "Undo mark on C22" }).click();
  await expect(entry).toContainText(/Undone .* by alice/);
  await page.getByRole("link", { name: "Questions, 11 need you" }).click();
  await expect(queue.getByRole("button", { name: /^C22/ })).toBeVisible();
});

test("a gate mark is recorded alone from the table", async ({ page }) => {
  await page.goto(`${PR}/comments/table?scenario=comments-v14`);
  await page.getByRole("combobox", { name: /^Decision for C14/ }).selectOption("not_a_review_point");
  await page.getByRole("textbox", { name: /^Reason or target for C14/ }).fill("Fixture only.");
  await expectAccessible(page);
  await page.getByRole("button", { name: "Mark C14" }).click();
  await expect(page.getByText("C14: Not a review point.", { exact: true })).toBeVisible();
  await expect(page.getByRole("combobox", { name: /^Decision for C14/ })).toHaveCount(0);
  await expect(page.getByRole("combobox", { name: /^Decision for C15/ })).toBeVisible();
  const provenance = /Not a review point · follows C14 · by alice · 09:\d{2}/;
  await page.getByRole("button", { name: "Show all", exact: true }).click();
  await expect(page.locator('tr[data-row="C30"]')).toContainText(provenance);

  await page.getByRole("link", { name: /^Questions,/ }).click();
  const queue = page.getByRole("navigation", { name: "Queue" });
  await queue.getByRole("button", { name: /^Status:/ }).click();
  await page.getByRole("menuitemradio", { name: "Not a review point (3)" }).click();
  const follower = queue.getByRole("button", { name: /^C30/ });
  await expect(follower).toContainText(provenance);
  await follower.click();
  await expect(page.getByRole("article", { name: "C30" })).toContainText(provenance);
  await page.getByRole("link", { name: "Decision log and agents", exact: true }).click();
  await expect(page.getByRole("listitem").filter({ hasText: /C14 · Not a review point/ })).toContainText(
    "Fixture only.",
  );
});

test("Settings lists marked items across cursor pages", async ({ page }) => {
  await page.goto("/settings/review-loop?scenario=comments-marks");
  const marks = page.getByRole("region", { name: "Not a review point" });
  await expect(marks.getByText("Review item 100", { exact: true })).toBeVisible();
  await expect(marks.getByText("Review item 101", { exact: true })).toHaveCount(0);
  await marks.getByRole("button", { name: "Show older marked items" }).click();
  await expect(marks.getByText("Review item 100", { exact: true })).toBeVisible();
  await expect(marks.getByText("Review item 101", { exact: true })).toBeVisible();
  await expectAccessible(page);
});
