// What My work shows of one ranked snapshot of the world (v1.1 plan §F): excluded items with their reason, a PR
// waiting only on others outside Needs you, change-size pills and the "Sizes shown" filter, the kind toggles
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
  await expect(workList(page).locator("li[data-item]")).toHaveCount(19);
});

test("Excluded lists the excluded author's pull request with its reason", async ({ page }) => {
  await signIn(page);
  const disclosure = page.getByRole("button", { name: "Excluded: 1 — 1 excluded author" });
  await expect(disclosure).toHaveAttribute("aria-expanded", "false");
  await disclosure.click();
  await expect(disclosure).toHaveAttribute("aria-expanded", "true");
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
  expect(await rowIds(page)).not.toContain("PLAT-1176");
  await page.getByRole("button", { name: /^« More · waiting 1 · ready 1$/ }).click();
  const waiting = page.getByRole("listitem").filter({ hasText: "PLAT-1176" });
  await expect(waiting).toHaveCount(1);
  await expect(waiting).toContainText("@jun-park");
  await expect(waiting).toContainText("team:core");
  await checkScreen(page);
});

test("size pills; Sizes shown hides a size of pull requests only", async ({ page }) => {
  await signIn(page);
  const row = (id: string) => workList(page).locator("li[data-item]", { hasText: id });
  await expect(row("WUI-320").getByLabel("Change size: tiny, 40 changed lines")).toBeVisible();
  await expect(row("NC-482").getByLabel(/^Change size: small, /)).toBeVisible();
  await expect(row("PLAT-1187").getByLabel(/^Change size: /)).toHaveCount(0);

  const sizes = workList(page).getByRole("group", { name: "Sizes shown" });
  const tiny = sizes.getByRole("button", { name: /^tiny\b/ });
  await expect(tiny).toHaveAttribute("aria-pressed", "true");
  await tiny.click();
  await expect(tiny).toHaveAttribute("aria-pressed", "false");
  const shown = await rowIds(page);
  for (const id of ["WUI-320", "OPS-14", "NC-476", "NC-490", "CON-1541"]) expect(shown).not.toContain(id);
  expect(shown).toEqual(expect.arrayContaining(["NC-482", "PLAT-1203", ...ISSUES, "TODO-3fa91c2e"]));
  await checkScreen(page);
  await tiny.click();
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
