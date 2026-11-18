import { expect, test } from "@playwright/test";
import { expectAccessible } from "./screens-support";
import { ANALYSIS_ID } from "./support";

test("rating and remark pills in the list and above the code lead to the first remark", async ({ page }) => {
  await page.goto(`/analyses/${ANALYSIS_ID}?scenario=remark-pills#f1.4`);
  const nav = page.getByRole("navigation");
  // A flow row: its highest step rating ×steps at it and all its remarks, also while collapsed (flow 2 has an
  // unrated step whose remarks still count).
  await expect(nav.getByRole("button", { name: /^LLM rated 1 step in flow 2 MEDIUM/ })).toHaveText(
    "◆ MEDIUM ×1 · 5 remarks",
  );
  // A rated step without remarks shows its rating alone; step 5 its rating and remarks as separate facts.
  await expect(nav.getByRole("button", { name: "LLM rated step 3 LOW (not checked)" })).toHaveText("● LOW");
  await expect(
    nav.getByRole("button", { name: "LLM rated step 5 HIGH (not checked) · 3 LLM remarks" }),
  ).toHaveText("▲ HIGH · 3 remarks");
  const flow1 = nav.getByRole("button", { name: /^LLM rated 1 step in flow 1 HIGH/ });
  expect((await flow1.boundingBox())!.height).toBeGreaterThanOrEqual(24);
  await flow1.focus();
  await page.keyboard.press("Enter");
  const main = page.getByRole("main");
  await expect(main.getByRole("heading", { level: 2, name: /Retry allowed/ })).toBeVisible();
  await expect(main.getByRole("button", { name: /^LLM rated this step HIGH/ })).toHaveText(
    "▲ HIGH · 3 remarks",
  );
  await expect(main.locator("[data-flash]").first()).toBeVisible();
  await expect(
    page.getByRole("complementary", { name: "Inspector" }).locator("[data-flash]").first(),
  ).toBeVisible();
  // Remarks on an unrated step: a neutral pill.
  await nav.getByRole("button", { name: /^Flow 2:/ }).click();
  await expect(nav.getByRole("button", { name: "LLM did not rate step 2 · 2 LLM remarks" })).toHaveText(
    "2 remarks",
  );
  // Every pill explains itself (the code view's line buttons are outside this check).
  for (const pill of await page.getByRole("button", { name: /^LLM (rated|did not rate)/ }).all())
    expect(await pill.getAttribute("aria-describedby")).toBeTruthy();
  await expectAccessible(page);
});
