import { expect, test } from "./fixtures";
import { expectTrustWording } from "./support";
import { expectAccessible } from "./screens-support";

test("link, observe the fake plan, and explicitly unlink from Settings", async ({ page }) => {
  const writes: string[] = [];
  page.on("request", (request) => {
    if (request.method() !== "GET" && request.url().includes("/api/"))
      writes.push(new URL(request.url()).pathname);
  });
  await page.goto("/settings");
  const card = page.getByRole("region", { name: "Codex account" });
  await expect(card).toContainText("Linking may log your Codex out on other machines");
  await expect(card.getByRole("button", { name: "Link Codex account", exact: true })).toBeEnabled();
  expect(writes).not.toContain("/api/v1/settings/codex/link");
  await card.getByRole("button", { name: "Link Codex account", exact: true }).click();
  await expect(card).toContainText("FAKE-PLAN");
  await expect(card).toContainText("Linked: alice@example.invalid");
  expect(writes.filter((path) => path === "/api/v1/settings/codex/link")).toHaveLength(1);
  await card.getByRole("button", { name: "Unlink Codex account", exact: true }).click();
  await expect(card).toContainText("Running turns will end");
  await card.getByRole("button", { name: "Keep linked" }).click();
  await expect(card).toContainText("Linked: alice@example.invalid");
  await card.getByRole("button", { name: "Unlink Codex account", exact: true }).click();
  await card.getByRole("button", { name: "Confirm unlink" }).click();
  await expect(card.getByRole("button", { name: "Link Codex account", exact: true })).toBeEnabled();
  await expectTrustWording(page);
  await expectAccessible(page);
});

test("PR A/B opt-in preserves primary, quota and visible comments; report is read-only", async ({ page }) => {
  const assessmentWrites: { path: string; body: unknown }[] = [];
  page.on("request", (request) => {
    const path = new URL(request.url()).pathname;
    // RC16 previews the current decision form without storing or scheduling work.
    if (request.method() !== "GET" && path.startsWith("/api/") && !path.endsWith("/comments/effects-preview"))
      assessmentWrites.push({ path, body: request.postDataJSON() });
  });
  await page.goto("/pr/acme/node-client/482/comments");
  const toggle = page.getByRole("checkbox", { name: "Compare both methods on this PR" });
  await expect(toggle).not.toBeChecked();
  await expect(page.getByText(/A\/B costs roughly 2×/)).toBeVisible();
  const cards = page.getByRole("article");
  await expect(cards.first()).toBeVisible();
  const headings = await cards.getByRole("heading", { level: 2 }).allTextContents();
  const quotes = await cards.locator("[data-comment-text]").allTextContents();
  expect(headings.length).toBeGreaterThan(0);
  expect(quotes.length).toBeGreaterThan(0);
  const quota = page.getByText("79 of 80 daily passes remaining; resets at 2026-11-19T00:00:00Z (UTC).");
  await expect(quota).toBeVisible();
  await toggle.click();
  await expect(toggle).toBeChecked();
  await expect(toggle).toBeEnabled();
  await toggle.click();
  await expect(toggle).not.toBeChecked();
  await expect(toggle).toBeEnabled();
  expect(assessmentWrites).toEqual([
    {
      path: "/api/v1/settings/comment-assessment",
      body: {
        revision: JSON.stringify({ primary_method: "claudius_host", ab_prs: [], ab_until: null }),
        config: { primary_method: "claudius_host", ab_prs: ["acme/node-client#482"], ab_until: null },
      },
    },
    {
      path: "/api/v1/settings/comment-assessment",
      body: {
        revision: JSON.stringify({
          primary_method: "claudius_host",
          ab_prs: ["acme/node-client#482"],
          ab_until: null,
        }),
        config: { primary_method: "claudius_host", ab_prs: [], ab_until: null },
      },
    },
  ]);
  await expect(quota).toBeVisible();
  expect(await cards.getByRole("heading", { level: 2 }).allTextContents()).toEqual(headings);
  expect(await cards.locator("[data-comment-text]").allTextContents()).toEqual(quotes);
  await page.getByText("Assessment comparison (read-only)", { exact: true }).click();
  const report = page.getByRole("region", { name: "Assessment comparison report" });
  await expect(report.getByRole("table", { name: "Assessment items" })).toBeVisible();
  await expect(report).toContainText("Method failed");
  await expect(report).toContainText("Unmatched point");
  await expect(report.getByRole("button")).toHaveCount(0);
  await expectTrustWording(page);
});
