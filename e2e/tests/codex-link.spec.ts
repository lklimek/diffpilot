import { expect, test } from "../support/fixtures";

test("Settings links and relinks a plan through the isolated fake gateway", async ({ page, stack }) => {
  test.skip(!stack.stateDir, "requires an isolated fake stack");
  // The dev stack bootstraps a synthetic payer; all link/unlink operations here use its fake gateway.
  await page.goto("/");
  await page.getByRole("link", { name: "Sign in with GitHub" }).click();
  await expect(page.getByRole("banner")).toContainText("alice");
  await page.goto("/settings");
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
