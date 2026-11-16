import { expect, test } from "@playwright/test";
import { ANALYSIS_ID, expectTrustWording, openTour } from "./support";

const heading = (page: import("@playwright/test").Page) =>
  page.getByRole("main").getByRole("heading", { level: 2 }).first();

test("PR URL → PR context → flow 1 introduction → step 1 via j", async ({ page }) => {
  await page.goto("/");
  await page.getByLabel("GitHub pull request URL").fill("https://github.com/acme/node-client/pull/482");
  await page.getByRole("button", { name: "Analyze" }).click();
  await expect(page).toHaveURL(new RegExp(`/analyses/${ANALYSIS_ID}$`));
  await expect(heading(page)).toHaveText("PR context: acme/node-client #482");

  await page.keyboard.press("j");
  await expect(heading(page)).toHaveText("1 Send an RPC request with retry");
  await expect(
    page.getByRole("navigation").getByRole("button", { name: "Flow 1: Send an RPC request with retry" }),
  ).toHaveAttribute("aria-current", "step");

  await page.keyboard.press("j");
  await expect(heading(page)).toHaveText("1 Trigger: callers of send");
  const code = page.getByRole("region", { name: "src/mempool/mod.rs, lines 126 to 132" });
  await expect(code).toContainText("pub async fn broadcast_tx");
  await expect(
    page.getByRole("navigation").getByRole("button", { name: "Step 1: Trigger: callers of send" }),
  ).toHaveAttribute("aria-current", "step");

  await page.keyboard.press("k");
  await expect(heading(page)).toHaveText("1 Send an RPC request with retry");
});

test("Previous / Next walk the sequence; after a flow's last step comes the next flow", async ({ page }) => {
  await openTour(page);
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Flow 1: Send an RPC request with retry" })
    .click();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /^Step 10:/ })
    .click();
  await expect(page.getByRole("button", { name: /^Next: Flow 2:/ })).toBeVisible();
  await page.getByRole("button", { name: /^Next: Flow 2:/ }).click();
  await expect(heading(page)).toContainText("Default timeout");
  await page.getByRole("button", { name: /^← Previous: Step 10:/ }).click();
  await expect(heading(page)).toContainText("Retry policy");
});

test("j / k are ignored while typing in a field", async ({ page }) => {
  await page.goto("/");
  const field = page.getByLabel("GitHub pull request URL");
  await field.fill("j");
  await field.press("j");
  await expect(field).toHaveValue("jj");
});

test("step block: gutter marks, one highlight with was → is, collapsed assessment", async ({ page }) => {
  await openTour(page);
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Flow 1: Send an RPC request with retry" })
    .click();
  await page.getByRole("navigation").getByRole("button", { name: "Step 5: Retry allowed?" }).click();
  const mark = page.locator("[role=mark]");
  await expect(mark).toHaveCount(1);
  await mark.hover();
  await expect(page.getByRole("tooltip").filter({ visible: true })).toContainText("Was: ");
  const summary = page.getByText("Assessment (LLM)");
  await expect(page.locator("details")).not.toHaveAttribute("open");
  await summary.click();
  await expect(page.locator("details")).toHaveAttribute("open");
});

test("resume opens the last viewed block", async ({ page }) => {
  await page.goto(`/analyses/${ANALYSIS_ID}?state=resume`);
  await expect(heading(page)).toHaveText("5 Retry allowed?");
});

test("an unknown analysis shows a plain error", async ({ page }) => {
  await page.goto("/analyses/00000000-0000-0000-0000-000000000000");
  await expect(page.getByRole("alert")).toHaveText("This analysis does not exist.");
});

for (const state of ["default", "wrong", "missed", "noq", "noprofile", "resume"]) {
  test(`wording rules on the rendered blocks in state ${state}`, async ({ page }) => {
    await openTour(page, state);
    await page.getByRole("button", { name: "PR context" }).click();
    await expectTrustWording(page);
    for (const flow of [1, 2, 3, 4, 5]) {
      await page.getByRole("button", { name: new RegExp(`^Flow ${flow}:`) }).click();
      await expectTrustWording(page);
      const steps = page.getByRole("navigation").getByRole("button", { name: /^Step / });
      const count = await steps.count();
      for (let i = 0; i < count; i += 1) {
        await steps.nth(i).click();
        await expect(page.getByRole("status")).toHaveCount(0);
        await expectTrustWording(page);
      }
    }
  });
}

test("noq: the step says the LLM raised no questions", async ({ page }) => {
  await openTour(page, "noq");
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Flow 1: Send an RPC request with retry" })
    .click();
  await page.getByRole("navigation").getByRole("button", { name: "Step 5: Retry allowed?" }).click();
  await expect(
    page.getByText("The LLM raised no questions. That is not evidence of no defects."),
  ).toBeVisible();
});

test("noprofile: the PR context states the missing project profile", async ({ page }) => {
  await openTour(page, "noprofile");
  await expect(page.getByText("No project profile — ratings are component-level")).toBeVisible();
});
