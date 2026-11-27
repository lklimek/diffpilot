import { expect, test } from "./fixtures";
import { expectAccessible, expectTooltips } from "./screens-support";
import { expectTrustWording } from "./support";

test("repository rules retain stored values and show App installation facts", async ({ page }) => {
  await page.goto("/settings/repositories");
  const table = page.getByRole("table", { name: "Repositories", exact: true });
  await expect(table.getByLabel("Short name of acme/platform")).toHaveValue("PLAT");
  await expect(table.getByRole("row").filter({ hasText: "acme/platform" })).toContainText(
    "App not installed",
  );
  const installed = table.getByLabel("Short name of acme/live-only");
  await expect(installed).toHaveValue("");
  await expect(installed).toHaveAttribute("placeholder", "live-only");
  await installed.pressSequentially("live");
  await expect(installed).toHaveValue("LIVE");
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.getByText("Dashboard settings saved.")).toBeVisible();
  const mark = table.getByText("not visible to your GitHub token");
  await expect(mark).toHaveCount(1);
  await expectTooltips(page);
  await expectTrustWording(page);
  await expectAccessible(page);
  await page.emulateMedia({ colorScheme: "dark" });
  await expectAccessible(page);
});

test("GitHub sync validates scope and sends the whole record in one save", async ({ page }) => {
  await page.goto("/settings/github-sync");
  const scope = page.getByRole("region", { name: "Scope" });
  const linked = scope.getByLabel("Add a linked login");
  await linked.fill("alice");
  await linked.press("Enter");
  await expect(linked).toHaveAccessibleDescription(/your own login/);
  const days = scope.getByLabel("Look-back days");
  await days.fill("0");
  let puts = 0;
  const isSave = (request: { method: () => string; url: () => string }) =>
    request.method() === "PUT" && request.url().endsWith("/api/v1/settings/dashboard");
  page.on("request", (request) => {
    if (isSave(request)) puts += 1;
  });
  const save = page.getByRole("button", { name: "Save", exact: true });
  await save.click();
  await expect(days).toHaveAccessibleDescription(/whole number from 1 to 30/);
  expect(puts).toBe(0);
  await days.fill("10");
  await linked.fill("alice-work");
  await scope.getByRole("button", { name: "Add to linked logins" }).click();
  const nightly = page.getByRole("region", { name: "Nightly scan" });
  await expect(nightly).toContainText("Runs once a night");
  await nightly.getByLabel("Run nightly scan").uncheck();
  await nightly.getByLabel("Time of day").selectOption("07:30");
  const request = page.waitForRequest(isSave);
  await save.click();
  expect((await request).postDataJSON()).toMatchObject({
    lookback_days: 10,
    schedule_enabled: false,
    schedule_time: "07:30",
    repositories: expect.arrayContaining([expect.objectContaining({ alias: "PLAT" })]),
  });
  await expect(page.getByText("Dashboard settings saved.")).toBeVisible();
  expect(puts).toBe(1);
});

test("nightly scan explains an unset time zone", async ({ page }) => {
  await page.goto("/settings/github-sync?settings-scenario=defaults");
  const zone = page.getByLabel("Time zone");
  await expect(zone).toHaveAccessibleDescription(/nightly scan needs a time zone/);
  await zone.selectOption("Europe/Berlin");
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.getByText("Dashboard settings saved.")).toBeVisible();
});

test("Reset to defaults fills GitHub sync and keeps its time zone", async ({ page }) => {
  await page.goto("/settings/github-sync");
  await page.getByRole("button", { name: "Reset to defaults" }).click();
  await expect(page.getByText("Defaults filled in. Save to keep them.")).toBeVisible();
  await expect(page.getByLabel("Time zone")).toHaveValue("Europe/Warsaw");
  await expect(page.getByText("Unsaved changes")).toBeVisible();
});

test("MemCan not configured: the TODO switch is off and says why", async ({ page }) => {
  await page.goto("/settings/memcan?settings-scenario=memcan-off");
  const toggle = page.getByLabel("Show my MemCan TODOs");
  await expect(toggle).toBeDisabled();
  await expect(toggle).not.toBeChecked();
  await expect(toggle).toHaveAccessibleDescription(/MemCan is not configured on this server/);
});

test("GitHub sync shows the field named in the server refusal", async ({ page }) => {
  await page.goto("/settings/github-sync?settings-scenario=save-rejected");
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText('Not saved: Time zone: "Etc/Unknown" is not a known');
});

test("one Save applies the model, Ask effort and ranking effort", async ({ page }) => {
  await page.goto("/settings/llm");
  const llm = page.getByRole("region", { name: "Model and effort" });
  const effort = llm.getByLabel("Dashboard ranking effort");
  await expect(effort).toHaveValue("low");
  await effort.selectOption("high");
  await llm.getByLabel("Ask effort").selectOption("low");
  await llm.getByLabel("Model", { exact: true }).selectOption("gpt-6-astra");
  await page.getByRole("link", { name: "Repositories" }).click();
  const dialog = page.getByRole("alertdialog", { name: "Unsaved settings" });
  await dialog.getByRole("button", { name: "Stay" }).click();
  await llm.getByRole("button", { name: "Save", exact: true }).click();
  await expect(llm.getByRole("status")).toHaveText("Saved");
  await expect(llm.getByLabel("Model", { exact: true })).toHaveValue("gpt-6-astra");
  await expect(llm.getByLabel("Ask effort")).toHaveValue("low");
  await expect(effort).toHaveValue("high");
  await expect(llm).toContainText(/changed by alice at/);
});

test("ranking limit reset at the cap requires confirmation and preserves the per-refresh cap", async ({
  page,
}) => {
  await page.goto("/settings/llm?scenario=settings-v14-at-limit");
  const tile = page.getByRole("region", { name: "Ranking today" });
  await expect(tile).toContainText("120 of 120 items sent for judgement");
  await tile.getByRole("button", { name: "Reset my limits" }).click();
  await expect(tile).toContainText("120 of 120 items sent for judgement");
  await tile.getByRole("button", { name: "Reset today's count" }).click();
  await expect(tile).toContainText("0 of 120 items sent for judgement");
  await expect(tile).toContainText("Up to 48 per refresh");
});

test("repository edits block both another settings page and a full page load", async ({ page }) => {
  await page.goto("/settings/repositories");
  const issues = page.getByLabel("Watch issues of acme/node-client");
  await issues.check();
  await page.getByRole("link", { name: "GitHub sync" }).click();
  const ask = page.getByRole("alertdialog", { name: "Unsaved settings" });
  await ask.getByRole("button", { name: "Stay" }).click();
  await expect(page).toHaveURL(/\/settings\/repositories$/);
  const asked = page.waitForEvent("dialog");
  void page.evaluate(() => {
    window.location.href = "/";
  });
  const dialog = await asked;
  expect(dialog.type()).toBe("beforeunload");
  await dialog.dismiss();
  await expect(issues).toBeChecked();
});

test("field help links to its anchors in the built-in guide", async ({ page }) => {
  await page.goto("/settings/repositories");
  const help = page.getByRole("link", { name: "Help: Release branches" });
  await help.focus();
  await expect(page.getByRole("tooltip")).toContainText("vX.Y-dev matches v1.8-dev and v5.1-dev");
  await expect(help).toHaveAttribute(
    "href",
    "/settings/help?from=%2Fsettings%2Frepositories#branch_patterns",
  );
  await page.goto("/settings/help?from=%2Fsettings%2Frepositories#ownership_policy");
  await expect(page.getByRole("heading", { level: 1, name: "Settings guide" })).toBeVisible();
  const policy = page.locator("section#ownership_policy");
  await expect(policy).toBeInViewport();
  await expect(
    policy.getByRole("link", { name: /^https:\/\/github\.com\/dashpay\/stale_prs_are_bad/ }),
  ).toHaveAttribute("target", "_blank");
  await page.getByRole("link", { name: "Back to Settings" }).click();
  await expect(page).toHaveURL(/\/settings\/repositories$/);
});
