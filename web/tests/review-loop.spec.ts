import type { Page } from "@playwright/test";
import { expect, test } from "./fixtures";
import { expectAccessible, expectTooltips } from "./screens-support";
import { expectTrustWording } from "./support";

// Review loop (docs/review-workflow-plan.md §3, §9.2 named Playwright tests), msw mode.
const PR = "/pr/acme/node-client/482";
const chat = (page: Page) => page.getByRole("dialog", { name: "Ask" });

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
  await expect(queue.getByRole("heading").last()).toHaveText("PR-level (1)");
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
