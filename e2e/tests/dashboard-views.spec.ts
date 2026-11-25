// What My work shows of one ranked snapshot: excluded items with their reason, a PR waiting only on others
// outside Needs you, change-size pills and filter chips, the kind toggles
// remembered over a reload; and Recent reviews on the review start page.
import { analyze, checkScreen, signIn } from "../support/app";
import {
  JUDGE_DATA,
  ranked,
  refreshButton,
  rowIds,
  saveSettings,
  seedTodos,
  waitForRefresh,
  workList,
  worldSettings,
} from "../support/dashboard";
import { expect, test } from "../support/fixtures";

test.describe.configure({ mode: "serial" });

const ISSUES = ["PLAT-1187", "PLAT-1210", "CON-1520", "CON-1525", "PLAT-1215"];

test("a ranked snapshot of the world", async ({ page, stack }) => {
  test.skip(!stack.memcan || !stack.github, "needs fake GitHub and fake MemCan (a local stack)");
  stack.secrets.push(...JUDGE_DATA);
  await signIn(page);
  await saveSettings(page, worldSettings());
  await seedTodos(page, stack);
  await refreshButton(page).click();
  await waitForRefresh(page, ranked);
  await expect(workList(page).locator("li[data-item]")).toHaveCount(18);
});

test("Excluded lists the excluded author's pull request with its reason", async ({ page }) => {
  await signIn(page);
  const status = page.getByRole("button", { name: "Status: All", exact: true });
  await expect(status).toHaveAttribute("aria-expanded", "false");
  expect(await rowIds(page)).not.toContain("NC-495");
  await status.click();
  await expect(status).toHaveAttribute("aria-expanded", "true");
  const excluded = page.getByRole("menuitemradio", { name: "Excluded and snoozed · 1" });
  await expect(excluded).toHaveAttribute("aria-checked", "false");
  await expect(excluded).toHaveAccessibleDescription("1 excluded author");
  await excluded.click();
  await expect(page.getByRole("button", { name: "Status: Excluded and snoozed" })).toHaveAttribute(
    "aria-expanded",
    "false",
  );
  const group = page.getByRole("group", { name: "Excluded authors: 1" });
  await expect(group).toContainText("Pull requests and issues written by an excluded author.");
  const row = group.getByRole("listitem");
  await expect(row).toHaveCount(1);
  await expect(row).toContainText("NC-495");
  await expect(row).toContainText("chore(deps): bump serde from 1.0.228 to 1.0.229");
  await expect(row).toContainText("by @dependabot[bot]");
  expect(await rowIds(page)).not.toContain("NC-495");
  await checkScreen(page);
});

test("a PR waiting only on others is not in Needs you but in Waiting on others", async ({ page }) => {
  await signIn(page);
  await expect(workList(page).locator("li[data-item]").first()).toBeVisible();
  const ranked = await rowIds(page);
  expect(ranked).not.toContain("PLAT-1176");
  expect(ranked).not.toContain("CON-1541");
  await page.getByRole("button", { name: /^« More · waiting 2 · ready 1$/ }).click();
  // The ready label (K15 W3a); a requested review, not approved, not mergeable by the user (W3c).
  const waiting = page.getByRole("listitem").filter({ hasText: "PLAT-1176" });
  await expect(waiting).toHaveCount(1);
  await expect(waiting).toContainText("@jun-park");
  await expect(waiting).toContainText("team:core");
  const requested = page.getByRole("listitem").filter({ hasText: "CON-1541" });
  await expect(requested).toHaveCount(1);
  await expect(requested).toContainText("@jun-park");
  await checkScreen(page);
});

test("size pills add a filter chip; excluding a size keeps issues and TODOs", async ({ page }) => {
  await signIn(page);
  const row = (id: string) => workList(page).locator("li[data-item]", { hasText: id });
  const tiny = row("WUI-320").getByRole("button", { name: "Filter by size tiny" });
  await expect(tiny).toBeVisible();
  await expect(tiny).toHaveAccessibleDescription(/40 changed lines/);
  const small = row("NC-482").getByRole("button", { name: "Filter by size small" });
  await expect(small).toBeVisible();
  await expect(small).toHaveAccessibleDescription(/changed lines/);
  await expect(row("PLAT-1187").getByRole("button", { name: /^Filter by size / })).toHaveCount(0);

  await tiny.click();
  const filters = workList(page).getByRole("region", { name: "Active filters" });
  const size = filters.getByRole("button", { name: "size: tiny" });
  await expect(size).toHaveAttribute("aria-pressed", "false");
  expect(await rowIds(page)).toEqual(expect.arrayContaining(["WUI-320", "OPS-14", "NC-476", "NC-490"]));
  expect(await rowIds(page)).not.toContain("NC-482");
  await size.click();
  await expect(size).toHaveAttribute("aria-pressed", "true");
  const shown = await rowIds(page);
  for (const id of ["WUI-320", "OPS-14", "NC-476", "NC-490"]) expect(shown).not.toContain(id);
  expect(shown).toEqual(expect.arrayContaining(["NC-482", "PLAT-1203", ...ISSUES, "TODO-3fa91c2e"]));
  await checkScreen(page);
  await filters.getByRole("button", { name: "Remove filter size tiny" }).click();
  await expect(row("WUI-320")).toBeVisible();
});

test("the kind toggles are remembered over a reload", async ({ page }) => {
  await signIn(page);
  const kinds = workList(page).getByRole("group", { name: "Kinds shown" });
  const issues = kinds.getByRole("button", { name: /^Issues\b/ });
  await expect(issues).toHaveAttribute("aria-pressed", "true");
  await issues.click();
  await expect(issues).toHaveAttribute("aria-pressed", "false");
  expect(await rowIds(page)).not.toEqual(expect.arrayContaining([ISSUES[0]]));
  await page.reload();
  await expect(workList(page).locator("li[data-item]").first()).toBeVisible();
  await expect(issues).toHaveAttribute("aria-pressed", "false");
  const shown = await rowIds(page);
  for (const id of ISSUES) expect(shown).not.toContain(id);
  expect(shown).toEqual(expect.arrayContaining(["PLAT-1203", "TODO-3fa91c2e"]));
  await issues.click();
  await page.reload();
  await expect(issues).toHaveAttribute("aria-pressed", "true");
  expect(await rowIds(page)).toEqual(expect.arrayContaining(ISSUES));
});

test("Recent reviews on the review start page lists the user's analysis", async ({ page }) => {
  await signIn(page);
  const id = await analyze(page);
  await page.goto("/review");
  const recent = page.getByRole("region", { name: "Recent reviews" });
  const link = recent.getByRole("link", { name: /^acme\/node-client #482\b/ });
  await expect(link).toHaveCount(1);
  await expect(link).toHaveAttribute("href", new RegExp(`/analyses/${id}(/progress)?$`));
  await checkScreen(page);
});
