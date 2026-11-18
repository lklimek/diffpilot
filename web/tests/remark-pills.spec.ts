import { expect, test } from "@playwright/test";
import { expectAccessible } from "./screens-support";
import { ANALYSIS_ID } from "./support";

test("rating and remark pills in the list and above the code lead to the first remark", async ({ page }) => {
  await page.goto(`/analyses/${ANALYSIS_ID}?scenario=remark-pills#f1.4`);
  const nav = page.getByRole("navigation");
  // A flow row: its rating once (its label) and a marker for all its remarks, also while collapsed (flow 2 has an
  // unrated step whose remarks still count).
  await expect(nav.getByRole("button", { name: "5 LLM remarks in flow 2" })).toHaveText("");
  // Steps show their rating only; the remark count is in the tooltip.
  await expect(nav.getByRole("button", { name: "LLM rated step 3 LOW (not checked)" })).toHaveText("● LOW");
  const step5 = nav.getByRole("button", { name: "LLM rated step 5 HIGH (not checked)" });
  await expect(step5).toHaveText("▲ HIGH");
  await expect(step5).toHaveAccessibleDescription(/^3 LLM remarks on step 5\./);
  const flow1 = nav.getByRole("button", { name: "18 LLM remarks in flow 1" });
  expect((await flow1.boundingBox())!.height).toBeGreaterThanOrEqual(24);
  await flow1.focus();
  await page.keyboard.press("Enter");
  const main = page.getByRole("main");
  await expect(main.getByRole("heading", { level: 2, name: /Retry allowed/ })).toBeVisible();
  await expect(main.getByRole("button", { name: /^LLM rated this step HIGH/ })).toHaveText("▲ HIGH");
  await expect(main.locator("[data-flash]").first()).toBeVisible();
  await expect(
    page.getByRole("complementary", { name: "Inspector" }).locator("[data-flash]").first(),
  ).toBeVisible();
  // Remarks on an unrated step: a small neutral marker, a 24 px target.
  await nav.getByRole("button", { name: /^Flow 2:/ }).click();
  const unrated = nav.getByRole("button", { name: "2 LLM remarks on step 2" });
  await expect(unrated).toHaveText("");
  await expect(unrated).toHaveAccessibleDescription(/the LLM did not rate it/);
  expect((await unrated.boundingBox())!.width).toBeGreaterThanOrEqual(24);
  // Every pill explains itself (the code view's line buttons are outside this check).
  for (const pill of await page.getByRole("button", { name: /^(LLM rated|\d+ LLM remarks? (in|on))/ }).all())
    expect(await pill.getAttribute("aria-describedby")).toBeTruthy();
  await expectAccessible(page);
});
