import { type Page } from "@playwright/test";
import { expect, test } from "./fixtures";
import { expectAccessible } from "./screens-support";
import { ANALYSIS_ID, expectTrustWording } from "./support";

// The floating Ask chat (proposal AC1–AC14): one private conversation, shared by Flows and Files.
const chat = (page: Page) => page.getByRole("dialog", { name: "Ask" });
const box = async (page: Page, selector: string) => (await page.locator(selector).first().boundingBox())!;

// AC2: the chat sits over the Inspector, above its dock, and never covers the centre column.
async function expectPlacement(page: Page) {
  const panel = (await chat(page).boundingBox())!;
  const centre = await box(page, '[data-column="main"]');
  const inspector = await box(page, '[data-column="inspector"]');
  const dock = await box(page, "[data-inspector] [data-dock]");
  expect(panel.x).toBeGreaterThanOrEqual(centre.x + centre.width - 1);
  expect(panel.x).toBeGreaterThanOrEqual(inspector.x - 1);
  expect(panel.y + panel.height).toBeLessThanOrEqual(dock.y + 1);
}

for (const [width, height] of [
  [1600, 900],
  [2560, 1080],
] as const) {
  test(`the chat stays clear of the code and the dock at ${width}x${height}`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    await page.goto(`/analyses/${ANALYSIS_ID}?scenario=ask-thread#f1.5`);
    await expect(page.getByRole("button", { name: /^Ask.*: open your private chat/ })).toBeVisible();
    await page.keyboard.press("a");
    await expect(chat(page)).toBeVisible();
    await expectPlacement(page);
    await chat(page).getByRole("button", { name: "Full height" }).click();
    await expectPlacement(page);
  });
}

test("a question, its follow-up and the subject across Flows and Files", async ({ page }) => {
  await page.setViewportSize({ width: 1600, height: 900 });
  await page.goto(`/analyses/${ANALYSIS_ID}?scenario=ask-thread#f1.5`);
  await page.getByRole("main").getByRole("button", { name: "Ask about this step" }).click();
  const panel = chat(page);
  await expect(panel.getByRole("textbox", { name: "Your question" })).toBeFocused();
  // Threads oldest first, the newest still being answered.
  await expect(panel.locator("[data-thread]")).toHaveCount(4);
  await expect(panel.getByText("2. The client retries.")).toBeVisible();
  await panel.getByRole("textbox", { name: "Your question" }).fill("Is the backoff capped?");
  await page.keyboard.press("Enter");
  const mine = panel.locator("[data-thread]").last();
  await expect(mine).toContainText("Is the backoff capped?");
  await expect(mine).toContainText("Mock answer", { timeout: 10_000 });
  await mine.getByRole("button", { name: "Ask a follow-up" }).click();
  await mine.getByRole("textbox", { name: /^Follow-up in this thread/ }).fill("And the jitter?");
  await page.keyboard.press("Enter");
  await expect(mine.getByRole("list", { name: "Follow-ups" })).toContainText("And the jitter?");
  // The Inspector keeps a read-only copy; Open in chat goes back to the thread.
  const copy = page
    .getByRole("complementary", { name: "Inspector" })
    .getByRole("region", { name: /^Your questions about this step/ });
  await expect(copy).toContainText("Is the backoff capped?");
  // To Files (s keeps the place): the same chat stays with its draft; the subject follows.
  await panel.getByRole("textbox", { name: "Your question" }).fill("draft");
  await page.getByRole("main").locator("h2:not([data-column-title])").first().click();
  await page.keyboard.press("s");
  await expect(page).toHaveURL(/\/files#lines\?/);
  await expect(chat(page).getByRole("textbox", { name: "Your question" })).toHaveValue("draft");
  await expect(chat(page).locator("[data-thread]")).toHaveCount(5);
  await page.goto(
    `/analyses/${ANALYSIS_ID}/files?scenario=ask-thread#lines?file=src/rpc/client.rs&side=head&line=100&end=102`,
  );
  await expect(chat(page)).toContainText(/About:\s*Files\s*src\/rpc\/client\.rs:100–102/);
  await expectTrustWording(page);
  await expectAccessible(page);
  // Esc hides it; the launcher brings it back.
  await chat(page).getByRole("textbox", { name: "Your question" }).focus();
  await page.keyboard.press("Escape");
  await expect(chat(page)).toBeHidden();
  await page.getByRole("button", { name: /^Ask.*: open your private chat/ }).click();
  await expect(chat(page)).toBeVisible();
});

// Light and dark at the two widths of the self-test, into SCREENSHOT_DIR (e.g. /data/tmp/ask-chat-web/shots).
const dir = process.env.SCREENSHOT_DIR;
for (const colorScheme of ["light", "dark"] as const) {
  for (const [width, height] of [
    [1600, 900],
    [2560, 1080],
  ] as const) {
    for (const [name, url] of [
      ["flows", `/analyses/${ANALYSIS_ID}?scenario=ask-thread#f1.5`],
      [
        "files",
        `/analyses/${ANALYSIS_ID}/files?scenario=ask-thread#lines?file=src/rpc/client.rs&side=head&line=100&end=102`,
      ],
    ] as const) {
      test(`ask chat ${name} ${colorScheme} ${width}x${height}`, async ({ browser }) => {
        test.skip(!dir, "SCREENSHOT_DIR not set");
        const page = await browser.newPage({ colorScheme, viewport: { width, height } });
        await page.goto(url);
        await expect(page.getByRole("button", { name: /^Ask.*: open your private chat/ })).toBeVisible();
        await page.screenshot({ path: `${dir}/ask-${name}-launcher-${colorScheme}-${width}.png` });
        await page.keyboard.press("a");
        await expect(chat(page)).toBeVisible();
        await page.waitForTimeout(300);
        await page.screenshot({ path: `${dir}/ask-${name}-${colorScheme}-${width}.png` });
        await page.close();
      });
    }
  }
}
