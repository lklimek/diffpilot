import { expect, test } from "../support/fixtures";

test("Settings links and relinks a plan through the isolated fake gateway", async ({ page, stack }) => {
  test.skip(!stack.stateDir, "requires an isolated fake stack");
  // The dev stack bootstraps a synthetic payer; all link/unlink operations here use its fake gateway.
  await page.goto("/");
  await page.getByRole("link", { name: "Sign in with GitHub" }).click();
  await expect(page.getByRole("banner")).toContainText("alice");
  await page.goto("/settings/llm");
  const card = page.getByRole("region", { name: "Codex account" });
  await expect(card).toContainText("Linking may log your Codex out on other machines");
  await card.getByRole("button", { name: "Unlink Codex account", exact: true }).click();
  await card.getByRole("button", { name: "Confirm unlink" }).click();
  await expect(card.getByRole("button", { name: "Link Codex account", exact: true })).toBeEnabled();
  await card.getByRole("button", { name: "Link Codex account", exact: true }).click();
  await expect(card).toContainText("Linked: Fake payer");
  await expect(card).toContainText("Plan: fake");
  await card.getByRole("button", { name: "Relink Codex account", exact: true }).click();
  await expect(card).toContainText("Linked: Fake payer");
  await expect(card.getByRole("button", { name: "Relink Codex account", exact: true })).toBeEnabled();
  await card.getByRole("button", { name: "Unlink Codex account", exact: true }).click();
  await card.getByRole("button", { name: "Confirm unlink" }).click();
  await expect(card.getByRole("button", { name: "Link Codex account", exact: true })).toBeEnabled();
  const status = await (await page.request.get("/api/v1/settings/codex")).json();
  expect(status.state).toBe("relink_needed");
  expect(JSON.stringify(status)).not.toMatch(/access_token|refresh_token|id_token|api_key/);
});

test("two stale settings editors cannot replace each other's A/B selections", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("link", { name: "Sign in with GitHub" }).click();
  await expect(page.getByRole("banner")).toContainText("alice");
  const route = "/api/v1/settings/comment-assessment";
  const initial = await (await page.request.get(route)).json();
  const me = await (await page.request.get("/api/v1/me")).json();
  const first = { ...initial.config, ab_prs: ["acme/first#1"] };
  const second = { ...initial.config, ab_prs: ["acme/second#2"] };
  const headers = { "X-CSRF-Token": me.csrf_token };
  const saved = await page.request.put(route, {
    headers,
    data: { revision: initial.revision, config: first },
  });
  expect(saved.status()).toBe(200);
  const stale = await page.request.put(route, {
    headers,
    data: { revision: initial.revision, config: second },
  });
  expect(stale.status()).toBe(409);
  const current = await (await page.request.get(route)).json();
  expect(current.config).toEqual(first);
  expect(current.quota).toEqual(initial.quota);
  const merged = { ...current.config, ab_prs: [...current.config.ab_prs, "acme/second#2"] };
  expect(
    (
      await page.request.put(route, { headers, data: { revision: current.revision, config: merged } })
    ).status(),
  ).toBe(200);
  expect((await (await page.request.get(route)).json()).config.ab_prs).toEqual([
    "acme/first#1",
    "acme/second#2",
  ]);
});
