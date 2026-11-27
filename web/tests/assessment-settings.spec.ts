import { expect, test } from "./fixtures";
import { expectTrustWording } from "./support";
import { expectAccessible } from "./screens-support";

test("link, observe the fake plan, and explicitly unlink from Settings", async ({ page }) => {
  const writes: string[] = [];
  page.on("request", (request) => {
    if (request.method() !== "GET" && request.url().includes("/api/"))
      writes.push(new URL(request.url()).pathname);
  });
  await page.goto("/settings/llm");
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
  const quota = page.getByText("79 of 80 daily passes remaining · resets at 19 Nov 2026, 00:00");
  await expect(quota).toBeVisible();
  await toggle.click();
  await expect(toggle).toBeChecked();
  await expect(toggle).toBeEnabled();
  await toggle.click();
  await expect(toggle).not.toBeChecked();
  await expect(toggle).toBeEnabled();
  expect(assessmentWrites).toEqual([
    { path: "/api/v1/prs/acme/node-client/482/comments/visit", body: null },
    { path: "/api/v1/prs/acme/node-client/482/comments/assess", body: null },
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

test("Settings keeps a draft after a concurrent primary and deadline update", async ({ page }) => {
  await page.goto("/settings/review-loop");
  const card = page.getByRole("region", { name: "Comment assessment" });
  await expect(card.getByLabel("Primary assessment method")).toHaveValue("claudius_host");
  await card.getByLabel("Primary assessment method").selectOption("prompt_v4");
  await card.getByLabel("Optional A/B deadline (UTC)").fill("2026-11-30T00:00:00Z");
  const changed = await page.evaluate(async () => {
    const { csrf_token } = await (await fetch("/api/v1/me")).json();
    const settings = await (await fetch("/api/v1/settings/comment-assessment")).json();
    const response = await fetch("/api/v1/settings/comment-assessment", {
      method: "PUT",
      headers: { "Content-Type": "application/json", "X-CSRF-Token": csrf_token },
      body: JSON.stringify({
        revision: settings.revision,
        config: { primary_method: "prompt_v4", ab_until: "2026-12-01T00:00:00Z", ab_prs: ["acme/other#10"] },
      }),
    });
    return response.status;
  });
  expect(changed).toBe(200);
  await card.getByRole("button", { name: "Save assessment settings" }).click();
  await expect(card.getByRole("alert")).toContainText("Your edits are kept.");
  await expect(card).toContainText("A/B deadline changed elsewhere: none → 2026-12-01T00:00:00Z");
  await expect(card).toContainText("Primary method changed elsewhere: claudius_host → prompt_v4");
  await expect(card).toContainText("Selected PRs changed elsewhere: none → acme/other#10");
  await expect(card.getByLabel("Primary assessment method")).toHaveValue("prompt_v4");
  await expect(card.getByLabel("Optional A/B deadline (UTC)")).toHaveValue("2026-11-30T00:00:00Z");
  const saved = page.waitForRequest(
    (request) => request.method() === "PUT" && request.url().endsWith("/settings/comment-assessment"),
  );
  await card.getByRole("button", { name: "Save assessment settings" }).click();
  expect((await saved).postDataJSON()).toEqual({
    revision: JSON.stringify({
      primary_method: "prompt_v4",
      ab_until: "2026-12-01T00:00:00Z",
      ab_prs: ["acme/other#10"],
    }),
    config: { primary_method: "prompt_v4", ab_prs: ["acme/other#10"], ab_until: "2026-11-30T00:00:00.000Z" },
  });
  await expect(card.getByText("Assessment settings saved.")).toBeVisible();
  await expect(card.getByText(/changed elsewhere/)).toHaveCount(0);
  await expectTrustWording(page);
});
