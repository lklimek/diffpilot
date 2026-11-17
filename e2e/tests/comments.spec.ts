// Journey (c): comments into the user's GitHub pending review on fake GitHub — create, a second comment in the
// same pending review, edit, delete, lines outside the diff, "missed by the LLM" (hard case 2).
import { type Page } from "@playwright/test";
import { checkScreen, finishedReview, inspector, navTo, openStep } from "../support/app";
import { expect, test } from "../support/fixtures";

test.describe.configure({ mode: "serial" });

interface Pending {
  commenting_available: boolean;
  comments: { id: string; path: string; kind: string; line: number | null; body: string }[];
}

/** The pending review as the server reads it from fake GitHub. */
async function pending(page: Page, id: string): Promise<Pending> {
  const response = await page.request.get(`/api/v1/analyses/${id}/pending-review`);
  expect(response.ok(), await response.text()).toBe(true);
  return (await response.json()) as Pending;
}

const pendingButton = (page: Page) =>
  page.getByRole("banner").getByRole("button", { name: /^Pending review/ });

async function comment(page: Page, line: number, location: string, body: string): Promise<void> {
  await page
    .getByRole("main")
    .getByRole("button", { name: `Comment on line ${line}`, exact: true })
    .click();
  await inspector(page)
    .getByRole("textbox", { name: `Comment on ${location}` })
    .fill(body);
  await inspector(page).getByRole("button", { name: "Add to GitHub pending review" }).click();
}

test("hard case 2: a comment on a changed line without LLM remark reaches GitHub and records missed by the LLM", async ({
  page,
}) => {
  const id = await finishedReview(page);
  await openStep(page, 1, 2);
  await comment(page, 96, "src/rpc/client.rs:96", "Is the attempt counter reset for every call?");
  const done = inspector(page).getByRole("status").filter({ hasText: "Added to your GitHub pending review" });
  await expect(done).toContainText("as a line comment");
  await expect(done).toContainText("Also recorded as missed by the LLM");
  await expect(pendingButton(page)).toContainText("1");
  expect((await pending(page, id)).comments).toMatchObject([
    {
      path: "src/rpc/client.rs",
      kind: "line",
      line: 96,
      body: "Is the attempt counter reset for every call?",
    },
  ]);
  const feedback = (await (await page.request.get(`/api/v1/analyses/${id}/feedback`)).json()) as {
    value: string;
  }[];
  expect(feedback.map((entry) => entry.value)).toContain("missed_by_llm");
  await expect(inspector(page)).toContainText("Recorded automatically: missed by the LLM");
  await expect(page.getByRole("main").locator("[data-mark=comment]")).toHaveCount(1);
  await checkScreen(page);
});

test("a second comment joins the same pending review; edit and delete act on GitHub", async ({ page }) => {
  const id = await finishedReview(page);
  await openStep(page, 1, 2);
  await comment(page, 91, "src/rpc/client.rs:91", "Rebuilt on every turn: intended?");
  const done = inspector(page).getByRole("status").filter({ hasText: "Added to your GitHub pending review" });
  await expect(done).toContainText("as a line comment");
  await expect(done).not.toContainText("missed by the LLM");
  await expect(pendingButton(page)).toContainText("2");
  const both = await pending(page, id);
  expect(both.comments.map((c) => c.line).sort()).toEqual([91, 96]);

  const mine = inspector(page).getByRole("listitem").filter({ hasText: "Rebuilt on every turn: intended?" });
  await mine.getByRole("button", { name: "Edit", exact: true }).click();
  const editor = inspector(page).getByRole("textbox", { name: "Edit comment" });
  await editor.fill("Rebuilt on every turn: is that needed?");
  // The editor open; the sweep leaves it open with the draft kept.
  await checkScreen(page);
  await expect(editor).toHaveValue("Rebuilt on every turn: is that needed?");
  await inspector(page).getByRole("button", { name: "Save", exact: true }).click();
  await expect
    .poll(async () => (await pending(page, id)).comments.map((c) => c.body))
    .toContain("Rebuilt on every turn: is that needed?");
  await checkScreen(page);

  const edited = inspector(page).getByRole("listitem").filter({ hasText: "is that needed?" });
  await edited.getByRole("button", { name: "Delete", exact: true }).click();
  // The delete confirmation open; the sweep leaves it open.
  await checkScreen(page);
  await edited.getByRole("button", { name: "Delete from GitHub" }).click();
  await expect.poll(async () => (await pending(page, id)).comments.map((c) => c.line)).toEqual([96]);
  await expect(pendingButton(page)).toContainText("1");
  await checkScreen(page);
});

test("a line outside the diff hunks becomes a file comment linking to the line", async ({ page }) => {
  const id = await finishedReview(page);
  await openStep(page, 2, 1);
  await page
    .getByRole("region", { name: /^src\/config\.rs, lines 12 to 26/ })
    .getByRole("button", { name: "Comment on line 12", exact: true })
    .click();
  await expect(inspector(page)).toContainText(
    "This line is outside the PR diff, so the comment is posted as a file comment whose text links to the line.",
  );
  await inspector(page)
    .getByRole("textbox", { name: "Comment on src/config.rs:12" })
    .fill("Doc comment outdated?");
  await inspector(page).getByRole("button", { name: "Add to GitHub pending review" }).click();
  await expect(
    inspector(page).getByRole("status").filter({ hasText: "Added to your GitHub pending review" }),
  ).toContainText("as a file comment that links to the line");
  const file = (await pending(page, id)).comments.find((c) => c.path === "src/config.rs");
  expect(file?.kind).toBe("file");
  expect(file?.body).toContain("Doc comment outdated?");
  expect(file?.body).toMatch(
    /https:\/\/github\.com\/acme\/node-client\/blob\/[0-9a-f]{40}\/src\/config\.rs#L12/,
  );
  await checkScreen(page);
});

test("a file outside the PR: a link to the line instead of a comment field", async ({ page }) => {
  await finishedReview(page);
  await openStep(page, 1, 1);
  await page
    .getByRole("region", { name: "src/mempool/mod.rs, lines 126 to 132" })
    .getByRole("button", { name: "Comment on line 126", exact: true })
    .click();
  await expect(inspector(page)).toContainText("This file is not part of the PR diff");
  const link = inspector(page).getByRole("link", { name: "line 126 on GitHub" });
  await expect(link).toHaveAttribute(
    "href",
    /^https:\/\/github\.com\/acme\/node-client\/blob\/[0-9a-f]{40}\/src\/mempool\/mod\.rs#L126$/,
  );
  await expect(inspector(page).getByRole("textbox", { name: /^Comment on / })).toHaveCount(0);
  await checkScreen(page);
});

test("security: comment text with markup renders as text", async ({ page }) => {
  const id = await finishedReview(page);
  const markup = '<img src=x onerror="window.__xss=1"><script>window.__xss=2</script>';
  await openStep(page, 1, 3);
  await comment(page, 97, "src/rpc/client.rs:97", markup);
  await expect(
    inspector(page).getByRole("status").filter({ hasText: "Added to your GitHub pending review" }),
  ).toBeVisible();
  expect((await pending(page, id)).comments.map((c) => c.body)).toContain(markup);
  await page.reload();
  await navTo(page, /^Pending review on GitHub/);
  await expect(page.getByRole("main").getByText(markup)).toBeVisible();
  await expect(page.locator("#root img, #root script")).toHaveCount(0);
  expect(await page.evaluate(() => (window as { __xss?: number }).__xss)).toBeUndefined();
  await checkScreen(page);
});

// TODO(comment_published): needs a review submitted on GitHub between the server's check and its write; fake
// GitHub refuses to submit reviews by design. Head moved (pr_head_changed): head-moved.spec.ts.
