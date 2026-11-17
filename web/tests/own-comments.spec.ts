import { expect, test, type Page } from "@playwright/test";
import { expectAccessible } from "./screens-support";
import { expectTrustWording, openTour } from "./support";

// Owner (real stack, flow 1 step 2): "I added a comment in step 1.2, went to 1.3, came back, and I cannot see it."
const nav = (page: Page) => page.getByRole("navigation");
const openStep = async (page: Page, flow: number, step: number) => {
  await nav(page)
    .getByRole("button", { name: new RegExp(`^Flow ${flow}:`) })
    .click();
  await nav(page)
    .getByRole("button", { name: new RegExp(`^Step ${step}:`) })
    .click();
};
const TEXT = "Each attempt gets the full timeout; is that intended?";
// The step's code view (the inspector's classic diff shows the same line with its own marker).
const code = (page: Page) => page.getByRole("region", { name: /^src\/rpc\/client\.rs, lines/ });

async function addComment(page: Page) {
  await openStep(page, 1, 2);
  await page.getByRole("button", { name: "Comment on line 96" }).click();
  await page.getByRole("textbox", { name: /^Comment on src\/rpc\/client\.rs:96/ }).fill(TEXT);
  await page.getByRole("button", { name: "Add to GitHub pending review" }).click();
  await expect(
    page.getByRole("status").filter({ hasText: "Added to your GitHub pending review" }),
  ).toBeVisible();
}

test("a comment added in a step is found again after Next and Previous: by mouse", async ({ page }) => {
  await openTour(page);
  await addComment(page);
  await page.getByRole("button", { name: "Next", exact: false }).first().click();
  await page.getByRole("button", { name: "← Previous" }).click();
  const marker = code(page).getByRole("button", { name: "Your pending comment at line 96" });
  await expect(marker).toBeVisible();
  await expect(marker).toHaveAccessibleDescription(/pending review; nothing is published/);
  await marker.click();
  const panel = code(page).getByRole("list", { name: "Your pending comments at line 96" });
  await expect(panel.getByText(TEXT)).toBeVisible();
  await expect(panel.getByRole("button", { name: /^Edit comment on/ })).toBeVisible();
  await expect(panel.getByRole("button", { name: /^Delete comment on/ })).toBeVisible();
  await expectTrustWording(page);
  for (const button of await panel.getByRole("button").all()) {
    await expect(button).toHaveAccessibleDescription(/GitHub/);
  }
  await expectAccessible(page);
});

test("… by keyboard, and after a reload", async ({ page }) => {
  await openTour(page);
  await addComment(page);
  await page.keyboard.press("Escape");
  await page.reload();
  const marker = code(page).getByRole("button", { name: "Your pending comment at line 96" });
  await expect(marker).toBeVisible();
  await marker.focus();
  await page.keyboard.press("Enter");
  await expect(
    code(page).getByRole("list", { name: "Your pending comments at line 96" }).getByText(TEXT),
  ).toBeVisible();
  await page.keyboard.press("Space");
  await expect(page.getByText(TEXT)).toHaveCount(0);
});

test("the pending-review block leads back to the step with the comment open", async ({ page }) => {
  await openTour(page, "missed");
  await nav(page)
    .getByRole("button", { name: /^Pending review on GitHub/ })
    .click();
  await page.getByRole("button", { name: /^Show in step 1\.2/ }).click();
  await expect(page.locator("main h2:not([data-column-title])").first()).toHaveText(/^2 /);
  await expect(code(page).getByRole("button", { name: "Your pending comment at line 96" })).toHaveAttribute(
    "aria-expanded",
    "true",
  );
});

// Light and dark at 1440 with SCREENSHOT_DIR set, e.g. /data/artifacts/smart-review/<date>/wp3-tour-8.
for (const colorScheme of ["light", "dark"] as const) {
  test(`screenshots ${colorScheme} 1440`, async ({ browser }) => {
    const dir = process.env.SCREENSHOT_DIR;
    test.skip(!dir, "SCREENSHOT_DIR not set");
    const page = await browser.newPage({ colorScheme, viewport: { width: 1440, height: 900 } });
    await openTour(page);
    await addComment(page);
    await page.getByRole("button", { name: "Next", exact: false }).first().click();
    await page.getByRole("button", { name: "← Previous" }).click();
    const marker = code(page).getByRole("button", { name: "Your pending comment at line 96" });
    await marker.hover();
    await expect(page.getByRole("tooltip").filter({ hasText: "nothing is published" })).toBeVisible();
    await page.screenshot({ path: `${dir}/marker-tooltip-${colorScheme}-1440.png` });
    await marker.click();
    await page.mouse.move(0, 0);
    await expect(code(page).getByText(TEXT)).toBeVisible();
    await page.screenshot({ path: `${dir}/comment-open-${colorScheme}-1440.png` });
    await nav(page)
      .getByRole("button", { name: /^Pending review on GitHub/ })
      .click();
    await expect(page.getByRole("button", { name: /^Show in step/ }).first()).toBeVisible();
    await page.screenshot({ path: `${dir}/pending-review-links-${colorScheme}-1440.png` });
    await page.close();
  });
}
