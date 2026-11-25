import { expect, test } from "./fixtures";
import { expectAccessible } from "./screens-support";

test("long PR Hygiene tooltip fits a 320 px viewport with 16 px gutters", async ({ page }) => {
  await page.setViewportSize({ width: 320, height: 900 });
  await page.goto("/?dashboard=mywork-details");
  const note = page.getByRole("note", { name: "PR Hygiene statement for NC-482" });
  await note.focus();
  const tip = page.getByRole("tooltip");
  await expect(tip).toContainText("Latest review by alice: Approved");
  const bounds = (await tip.boundingBox())!;
  expect(bounds.x).toBeGreaterThanOrEqual(16);
  expect(bounds.x + bounds.width).toBeLessThanOrEqual(304);
});

for (const colorScheme of ["light", "dark"] as const) {
  test(`My work Comments action and PR Hygiene context (${colorScheme})`, async ({ page }) => {
    await page.emulateMedia({ colorScheme });
    await page.goto("/?dashboard=mywork-details");
    const row = page.locator('li[data-item="acme~node-client~pr~482"]');
    const comments = row.getByRole("link", { name: "Comments of NC-482", exact: true });
    const review = row.getByRole("link", { name: /^Review pull request / });
    await expect(comments).toHaveAttribute("href", "/pr/acme/node-client/482/comments");
    expect((await comments.boundingBox())!.y).toBeGreaterThan((await review.boundingBox())!.y);
    await comments.focus();
    await expect(page.getByRole("tooltip")).toContainText(
      "assesses comments needing assessment with the LLM",
    );
    await page.keyboard.press("Escape");

    const note = row.getByRole("note", { name: "PR Hygiene statement for NC-482" });
    await note.hover();
    let tip = page.getByRole("tooltip");
    await expect(tip).toContainText("PR Hygiene: coderabbitai has not reported");
    await expect(tip).toContainText("Blocker: thepastaclaw requested changes");
    await expect(tip).toContainText("Latest review by alice: Approved");
    await expect(tip).toContainText("External text: <img src=x onerror=alert(1)>");
    await expect(tip.locator("a, img, button")).toHaveCount(0);
    await page.mouse.move(0, 0);
    await expect(tip).toHaveCount(0);
    await note.focus();
    tip = page.getByRole("tooltip");
    await expect(tip).toBeVisible();
    const bounds = (await tip.boundingBox())!;
    expect(bounds.y).toBeGreaterThanOrEqual(0);
    expect(bounds.y + bounds.height).toBeLessThanOrEqual(900);
    await page.keyboard.press("PageDown");
    await expect.poll(() => tip.evaluate((el) => el.scrollTop)).toBeGreaterThan(0);
    await page.keyboard.press("Escape");
    await expect(tip).toHaveCount(0);
    await expectAccessible(page);

    await comments.focus();
    await page.keyboard.press("Enter");
    await expect(page).toHaveURL("/pr/acme/node-client/482/comments");
    await expect(page.getByRole("navigation", { name: "Queue" })).toBeVisible();
  });
}
