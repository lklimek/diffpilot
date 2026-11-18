import { expect, test, type Page } from "@playwright/test";
import { expectAccessible, expectKeyboardTooltips, expectTooltips } from "./screens-support";
import { expectTrustWording } from "./support";

// Settings → Dashboard section and ranking effort (plan §F.6) against msw; `?settings-scenario=` picks the
// dashboard settings scenario (src/mocks/handlers/dashboard-settings.ts).

const dashboard = (page: Page) => page.getByRole("region", { name: "Dashboard", exact: true });

test("stored settings: rows, fallbacks as placeholders, unreachable mark, accessible in both themes", async ({
  page,
}) => {
  await page.goto("/settings");
  const card = dashboard(page);
  await expect(card.getByLabel("Short name of acme/platform")).toHaveValue("PLAT");
  await expect(card.getByLabel("Short name of acme/legacy-tools")).toHaveAttribute(
    "placeholder",
    "legacy-tools",
  );
  await expect(card.getByLabel("Release branches of acme/wallet-ui")).toHaveAttribute(
    "placeholder",
    "main, master, release/*, hotfix/*, vX.Y, vX.Y-dev",
  );
  const mark = card.getByText("not visible to your GitHub token");
  await expect(mark).toHaveCount(1);
  await mark.focus();
  await expect(page.getByRole("tooltip")).toContainText("could not see acme/legacy-tools");
  await expect(card.getByRole("list", { name: "MemCan projects", exact: true })).toContainText(
    "legacy-tools",
  );
  await expect(card.getByLabel("Time zone")).toHaveValue("Europe/Warsaw");
  await expectTooltips(page);
  await expectKeyboardTooltips(page);
  await expectTrustWording(page);
  await expectAccessible(page);
  await page.emulateMedia({ colorScheme: "dark" });
  await expectAccessible(page);
});

test("client validation names each field before anything is sent; a valid form saves", async ({ page }) => {
  await page.goto("/settings");
  const card = dashboard(page);
  const linked = card.getByLabel("Add a linked login");
  await linked.fill("alice");
  await linked.press("Enter");
  await expect(linked).toHaveAccessibleDescription(/your own login/);
  await card.getByLabel("Repository 2").fill("acme/platform");
  await card.getByLabel("Look-back days").fill("0");
  let puts = 0;
  page.on("request", (r) => {
    if (r.method() === "PUT" && r.url().endsWith("/api/v1/settings/dashboard")) puts += 1;
  });
  await card.getByRole("button", { name: "Save dashboard settings" }).click();
  const alert = card.getByRole("alert");
  await expect(alert).toContainText("Repository 2: acme/platform is listed twice.");
  await expect(alert).toContainText("Look-back days: use a whole number from 1 to 30.");
  await expect(card.getByLabel("Look-back days")).toHaveAccessibleDescription(/whole number from 1 to 30/);
  await expectAccessible(page);
  await card.getByLabel("Repository 2").fill("acme/wallet-ui");
  await card.getByLabel("Look-back days").fill("10");
  await card.getByLabel("Add a linked login").fill("alice-work");
  await card.getByRole("button", { name: "Add to linked logins" }).click();
  await card.getByLabel("Refresh my dashboard automatically every day").uncheck();
  await card.getByLabel("Time of day").selectOption("07:30");
  await card.getByRole("button", { name: "Save dashboard settings" }).click();
  await expect(card.getByRole("status")).toHaveText("Dashboard settings saved.");
  expect(puts).toBe(1);
  await expect(card.getByRole("list", { name: "Linked logins" })).toContainText("alice-work");
});

test("never saved: defaults, no time zone yet, an added row", async ({ page }) => {
  await page.goto("/settings?settings-scenario=defaults");
  const card = dashboard(page);
  await expect(card).toContainText("No repositories yet.");
  await expect(card.getByLabel("Time zone")).toHaveAccessibleDescription(/automatic refresh does not run/);
  await card.getByRole("button", { name: "Add repository" }).click();
  await card.getByLabel("Repository 1").fill("acme/web");
  await expect(card.getByLabel("Short name of acme/web")).toHaveAttribute("placeholder", "web");
  await card.getByLabel("Short name of acme/web").fill("web");
  await expect(card.getByLabel("Short name of acme/web")).toHaveValue("WEB");
  await card.getByLabel("Time zone").selectOption("Europe/Berlin");
  await card.getByRole("button", { name: "Save dashboard settings" }).click();
  await expect(card.getByRole("status")).toHaveText("Dashboard settings saved.");
  await expectAccessible(page);
});

test("Reset to defaults fills the form, keeps the time zone and saves only on Save", async ({ page }) => {
  await page.goto("/settings");
  const card = dashboard(page);
  await card.getByRole("button", { name: "Reset to defaults" }).click();
  await expect(card.getByRole("status")).toHaveText("Defaults filled in. Save to keep them.");
  await expect(card.getByRole("table", { name: "Repositories" })).toHaveCount(0);
  await expect(card.getByLabel("Time zone")).toHaveValue("Europe/Warsaw");
});

test("MemCan not configured: the TODO switch is off and says why", async ({ page }) => {
  await page.goto("/settings?settings-scenario=memcan-off");
  const toggle = dashboard(page).getByLabel("Show my MemCan TODOs");
  await expect(toggle).toBeDisabled();
  await expect(toggle).not.toBeChecked();
  await expect(toggle).toHaveAccessibleDescription(/MemCan is not configured on this server/);
});

test("the server's refusal is shown with the field it names", async ({ page }) => {
  await page.goto("/settings?settings-scenario=save-rejected");
  const card = dashboard(page);
  await card.getByRole("button", { name: "Save dashboard settings" }).click();
  await expect(card.getByRole("alert")).toContainText('Not saved: Time zone: "Etc/Unknown" is not a known');
});

test("ranking effort sits in the LLM card, saves on change and shows the editor", async ({ page }) => {
  await page.goto("/settings");
  const llm = page.getByRole("region", { name: "LLM model" });
  const effort = llm.getByLabel("Dashboard ranking effort");
  await expect(effort).toHaveValue("low");
  await effort.selectOption("high");
  await expect(page.getByText("Ranking effort saved.")).toBeVisible();
  await expect(llm.getByLabel("Dashboard ranking effort")).toHaveValue("high");
  await expect(llm).toContainText(/changed by alice at/);
});

test("Sign out still clears this login's drafts with the dashboard section on the page", async ({ page }) => {
  await page.goto("/settings");
  await expect(dashboard(page)).toBeVisible();
  await page.evaluate(() =>
    sessionStorage.setItem("smart-review.comment-drafts.alice.a1", JSON.stringify({ k: "mine" })),
  );
  await page.getByRole("button", { name: "Sign out" }).click();
  await expect(page).toHaveURL(/\/$/);
  expect(
    await page.evaluate(() => sessionStorage.getItem("smart-review.comment-drafts.alice.a1")),
  ).toBeNull();
});

test("unsaved changes are marked and leaving the page asks first", async ({ page }) => {
  await page.goto("/settings");
  const card = dashboard(page);
  await expect(card.getByText("Unsaved changes")).toHaveCount(0);
  await card.getByLabel("Watch issues of acme/node-client").check();
  await expect(card.getByText("Unsaved changes")).toBeVisible();
  // Inside the shell the brand link is an in-app move: the screen asks.
  await page.getByRole("link", { name: "smart-review" }).first().click();
  const ask = page.getByRole("alertdialog", { name: "Unsaved dashboard settings" });
  await ask.getByRole("button", { name: "Stay" }).click();
  await expect(ask).toHaveCount(0);
  await expect(page).toHaveURL(/\/settings/);
  await expect(card.getByLabel("Watch issues of acme/node-client")).toBeChecked();
  // Leaving the app (a full page load) asks through the browser.
  const asked = page.waitForEvent("dialog");
  void page.evaluate(() => {
    window.location.href = "/";
  });
  const dialog = await asked;
  expect(dialog.type()).toBe("beforeunload");
  await dialog.dismiss();
  await expect(page).toHaveURL(/\/settings/);
  await expect(card.getByLabel("Watch issues of acme/node-client")).toBeChecked();
});
