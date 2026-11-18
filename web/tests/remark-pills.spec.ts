import { expect, test } from "@playwright/test";
import { expectAccessible } from "./screens-support";
import { ANALYSIS_ID } from "./support";

test("LLM remark pills in the list and above the code lead to the first remark of their band", async ({
  page,
}) => {
  await page.goto(`/analyses/${ANALYSIS_ID}?scenario=remark-pills#f1.4`);
  const nav = page.getByRole("navigation");
  // Collapsed flows show their totals; an unrated step's remarks are not counted, a step without any shows none.
  await expect(nav.getByRole("group", { name: "LLM remarks in flow 2" }).getByRole("button")).toHaveText([
    "◆ MED 3",
  ]);
  await expect(nav.getByRole("group", { name: "LLM remarks on step 3" })).toHaveCount(0);
  await expect(nav.getByRole("group", { name: "LLM remarks on step 5" }).getByRole("button")).toHaveText([
    "▲ HIGH 3",
  ]);
  const flow1 = nav.getByRole("group", { name: "LLM remarks in flow 1" });
  const high = flow1.getByRole("button", { name: /HIGH 3/ });
  expect((await high.boundingBox())!.height).toBeGreaterThanOrEqual(24);
  await high.focus();
  await page.keyboard.press("Enter");
  const main = page.getByRole("main");
  await expect(main.getByRole("heading", { level: 2, name: /Retry allowed/ })).toBeVisible();
  await expect(main.getByRole("group", { name: "LLM remarks on this step" })).toContainText(
    "Remarks on this step:",
  );
  await expect(main.locator("[data-flash]").first()).toBeVisible();
  await expect(
    page.getByRole("complementary", { name: "Inspector" }).locator("[data-flash]").first(),
  ).toBeVisible();
  // Every pill explains itself (the code view's line buttons are outside this check).
  for (const pill of await page.locator("[role=group][aria-label^='LLM remarks'] button").all())
    expect(await pill.getAttribute("aria-describedby")).toBeTruthy();
  await expectAccessible(page);
});
