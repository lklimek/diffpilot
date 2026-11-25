import { expect, test } from "@playwright/test";
import { expectTrustWording } from "./support";

test("user menu shows LLM work, preserves job privacy and opens keyboard help", async ({ page }) => {
  await page.goto("/?scenario=llm-activity-running");
  const banner = page.getByRole("banner");
  const trigger = banner.getByRole("button", { name: "alice", exact: true });
  await trigger.focus();
  await trigger.press("Enter");
  await expect(trigger).toHaveAttribute("aria-expanded", "true");
  await expect(banner.getByRole("list", { name: "Running now (1)" })).toContainText("Ranking");
  await expect(banner.getByRole("list", { name: "Queued (2)" })).toContainText("1 job of another user");
  await expect(banner.getByRole("link", { name: "acme/node-client#482" })).toBeVisible();
  await expect(trigger).not.toHaveText(/⏸|✕/);
  await expectTrustWording(page);
  await page.keyboard.press("Escape");
  await expect(trigger).toBeFocused();

  await trigger.click();
  await banner.getByRole("button", { name: "Keyboard shortcuts" }).click();
  const help = page.getByRole("dialog", { name: "Keyboard shortcuts" });
  await expect(help).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(trigger).toBeFocused();
  await page.keyboard.press("?");
  await expect(help).toBeVisible();
});

test("the disclosed job lists fit a phone in dark mode and can be expanded", async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 640 });
  await page.emulateMedia({ colorScheme: "dark" });
  await page.goto("/?scenario=llm-activity-many");
  const banner = page.getByRole("banner");
  await banner.getByRole("button", { name: "alice", exact: true }).click();
  const jobs = banner.getByRole("list", { name: "Queued (7)" });
  await expect(jobs.getByRole("listitem")).toHaveCount(5);
  await banner.getByRole("button", { name: "Show all (7)" }).click();
  await expect(jobs.getByRole("listitem")).toHaveCount(7);
  const panel = banner.getByRole("link", { name: "Usage and limits" });
  const bounds = await banner.getByRole("region", { name: "LLM activity" }).boundingBox();
  expect(bounds).not.toBeNull();
  expect(bounds!.x).toBeGreaterThanOrEqual(0);
  expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(360);
  await panel.click();
  await expect(page).toHaveURL(/\/settings#llm-usage$/);
});

test("the trigger identifies a usage pause and a failed last call", async ({ page }) => {
  await page.goto("/?scenario=llm-activity-paused");
  const paused = page.getByRole("button", { name: /alice — LLM paused until/ });
  await paused.hover();
  await expect(page.getByRole("tooltip").filter({ hasText: /LLM paused until/ })).toBeVisible();
  await paused.click();
  await expect(page.getByText(/Paused at the usage limit until/)).toBeVisible();

  await page.goto("/?scenario=llm-activity-failed");
  await page.getByRole("button", { name: "alice — LLM not working" }).click();
  await expect(page.getByText("Codex sign-in expired.")).toBeVisible();
});
