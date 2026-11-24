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
  const assessmentWrites: string[] = [];
  page.on("request", (request) => {
    const path = new URL(request.url()).pathname;
    // RC16 previews the current decision form without storing or scheduling work.
    if (request.method() !== "GET" && path.startsWith("/api/") && !path.endsWith("/comments/effects-preview"))
      assessmentWrites.push(path);
  });
  await page.goto("/pr/acme/node-client/482/comments");
  const toggle = page.getByRole("checkbox", { name: "Compare both methods on this PR" });
  await expect(toggle).not.toBeChecked();
  await expect(page.getByText(/A\/B costs roughly 2×/)).toBeVisible();
  await toggle.click();
  await expect(toggle).toBeChecked();
  await expect(toggle).toBeEnabled();
  await toggle.click();
  await expect(toggle).not.toBeChecked();
  await expect(toggle).toBeEnabled();
  expect(assessmentWrites).toEqual([
    "/api/v1/settings/comment-assessment",
    "/api/v1/settings/comment-assessment",
  ]);
  await page.getByText("Assessment comparison (read-only)", { exact: true }).click();
  const report = page.getByRole("region", { name: "Assessment comparison report" });
  await expect(report.getByRole("table", { name: "Assessment items" })).toBeVisible();
  await expect(report).toContainText("Method failed");
  await expect(report).toContainText("Unmatched point");
  await expect(report.getByRole("button")).toHaveCount(0);
  await expectTrustWording(page);
});
