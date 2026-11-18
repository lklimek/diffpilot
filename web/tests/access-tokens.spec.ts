import { expect, test, type Page } from "@playwright/test";
import { expectAccessible, expectKeyboardTooltips, expectTooltips } from "./screens-support";
import { expectTrustWording } from "./support";

// Settings → Access tokens (ADR-019, plan §F.6) against msw; `?settings-scenario=` as in settings-dashboard.

const tokens = (page: Page) => page.getByRole("region", { name: "Access tokens" });

test.beforeEach(async ({ page }) => {
  await page.clock.setFixedTime(new Date("2026-11-18T12:00:00Z"));
});

test("list says tokens are read-only, with prefix, scope and dates", async ({ page }) => {
  await page.goto("/settings");
  const card = tokens(page);
  await expect(card.getByRole("row", { name: /dispatcher agent/ })).toContainText("srp_Ab3dE5gH…");
  await expect(card.getByRole("row", { name: /laptop scripts/ })).toContainText("never");
  await expect(card).toContainText("read-only");
  await expect(card).toContainText("2 of 10 tokens in use.");
  await expectTooltips(page);
  await expectKeyboardTooltips(page);
  await expectTrustWording(page);
  await expectAccessible(page);
});

test("create: the secret is shown once with a copy button and is gone after Done", async ({
  page,
  context,
}) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.goto("/settings");
  await tokens(page).getByRole("button", { name: "New token" }).click();
  const form = page.getByRole("dialog", { name: "New access token" });
  await expect(form.getByLabel("Name")).toBeFocused();
  await expect(form.getByLabel("Expires after")).toHaveValue("90");
  await expect(form).toContainText("Scope: read");
  await expectAccessible(page);
  await form.getByLabel("Name").fill("ci agent");
  await form.getByLabel("Expires after").selectOption("30");
  await form.getByRole("button", { name: "Create token" }).click();
  const shown = page.getByRole("dialog", { name: "Token created" });
  await expect(shown).toContainText("Shown only now.");
  await expect(shown).toContainText("it cannot be shown again");
  const field = shown.getByLabel("Your new token");
  const secret = await field.inputValue();
  expect(secret).toMatch(/^srp_[a-z0-9]{8}_/);
  await shown.getByRole("button", { name: "Copy" }).click();
  await expect(shown.getByRole("status")).toHaveText("Copied to the clipboard.");
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(secret);
  await expectAccessible(page);
  await page.emulateMedia({ colorScheme: "dark" });
  await expectAccessible(page);
  // Escape (here dismissing the Copy tooltip) never discards the secret: only Done closes this view.
  await shown.getByRole("button", { name: "Copy" }).focus();
  await page.keyboard.press("Escape");
  await expect(shown.getByLabel("Your new token")).toHaveValue(secret);
  await shown.getByRole("button", { name: "Done" }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(tokens(page).getByRole("row", { name: /ci agent/ })).toContainText("18 Dec 2026");
  expect(await page.content()).not.toContain(secret);
  const stored = await page.evaluate(() => JSON.stringify({ ...localStorage, ...sessionStorage }));
  expect(stored).not.toContain(secret);
});

test("the dialog keeps the keyboard inside and Escape closes it", async ({ page }) => {
  await page.goto("/settings");
  const opener = tokens(page).getByRole("button", { name: "New token" });
  await opener.click();
  const form = page.getByRole("dialog", { name: "New access token" });
  for (let i = 0; i < 6; i += 1) {
    await page.keyboard.press("Tab");
    expect(await form.evaluate((el) => el.contains(document.activeElement))).toBe(true);
  }
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(opener).toBeFocused();
});

test("a name in use is refused before sending", async ({ page }) => {
  await page.goto("/settings");
  await tokens(page).getByRole("button", { name: "New token" }).click();
  const form = page.getByRole("dialog", { name: "New access token" });
  await form.getByLabel("Name").fill("laptop scripts");
  await form.getByRole("button", { name: "Create token" }).click();
  await expect(form.getByRole("alert")).toContainText('already have a token called "laptop scripts"');
});

test("revoke asks for confirmation first", async ({ page }) => {
  await page.goto("/settings");
  const card = tokens(page);
  await card.getByRole("button", { name: "Revoke laptop scripts" }).click();
  const confirm = card.getByRole("group", { name: "Confirm revoking laptop scripts" });
  await confirm.getByRole("button", { name: "Keep" }).click();
  await expect(card.getByRole("row", { name: /laptop scripts/ })).toBeVisible();
  await card.getByRole("button", { name: "Revoke laptop scripts" }).click();
  await card.getByRole("button", { name: "Revoke token" }).click();
  await expect(card.getByRole("row", { name: /laptop scripts/ })).toHaveCount(0);
  await expect(card).toContainText("1 of 10 tokens in use.");
});

test("at 10 tokens New token is unavailable and says why", async ({ page }) => {
  await page.goto("/settings?settings-scenario=token-limit");
  const button = tokens(page).getByRole("button", { name: "New token" });
  await expect(button).toHaveAttribute("aria-disabled", "true");
  await expect(button).toHaveAccessibleDescription("You have 10 tokens. Revoke one to create another.");
  await button.click({ force: true });
  await expect(page.getByRole("dialog")).toHaveCount(0);
});

test("no tokens yet; tokens that cannot be read are said plainly", async ({ page }) => {
  await page.goto("/settings?settings-scenario=no-tokens");
  await expect(tokens(page)).toContainText("No tokens yet.");
  await page.goto("/settings?settings-scenario=load-fails");
  // The app retries a failed read a few times before it says so.
  await expect(tokens(page).getByRole("alert")).toContainText("cannot be read right now", {
    timeout: 20_000,
  });
});
