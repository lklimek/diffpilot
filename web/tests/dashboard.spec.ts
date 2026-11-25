import { type Page } from "@playwright/test";
import { pauseClock, settle } from "./clock-support";
import { expect, test } from "./fixtures";
import { expectAccessible, expectKeyboardTooltips, netChartBar, PR_URL } from "./screens-support";
import { ANALYSIS_ID } from "./support";

// My work at `/` (plan §F) against the msw dashboard scenarios
// (`?dashboard=<name>`, src/mocks/handlers/dashboard.ts).

/** One step of the mock's scripted refresh (`SCRIPT_STEP_MS` of src/mocks/handlers/dashboard.ts). */
const SCRIPT_STEP_MS = 500;

test.describe("My work (msw)", () => {
  test("mentions: three rows, bot preference, hide, count, keyboard and narrow themes", async ({ page }) => {
    await page.goto("/?dashboard=mentions");
    const strip = page.getByRole("region", { name: "Mentions" });
    await expect(strip.getByRole("link")).toHaveCount(0);
    await strip.getByRole("button", { name: "Mentions (4)" }).click();
    await expect(strip.getByRole("link")).toHaveCount(3);
    await expect(page.locator("[data-nav-count]")).toHaveText("4");
    await strip.getByRole("button", { name: "+1 more" }).click();
    await expect(strip.getByRole("link")).toHaveCount(4);
    await strip.getByRole("button", { name: "with bots", exact: true }).click();
    await expect(strip.getByText(/· bot/)).toBeVisible();
    await expect(page.locator("[data-nav-count]")).toHaveText("4");
    await page.reload();
    await expect(strip.getByRole("button", { name: "with bots", exact: true })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await strip
      .getByRole("button", { name: /Hide mention/ })
      .first()
      .focus();
    await page.keyboard.press("Enter");
    await expect(page.locator("[data-nav-count]")).toHaveText("3");
    await expect(
      page.getByRole("link", { name: "My work, 3 unanswered mentions from people" }),
    ).toBeVisible();
    await page.getByRole("link", { name: "My work, 3 unanswered mentions from people" }).hover();
    await expect(
      page.getByRole("tooltip", { name: /Your PRs, issues and TODOs.*3 unanswered mentions from people/ }),
    ).toBeVisible();
    for (const colorScheme of ["light", "dark"] as const) {
      await page.emulateMedia({ colorScheme });
      await page.setViewportSize({ width: 390, height: 844 });
      await expect(strip).toBeVisible();
      const nav = page.getByRole("navigation", { name: "Main" });
      await expect(page.locator("[data-nav-count]")).toBeVisible();
      await expect(nav.getByRole("button", { name: "Collapse navigation" })).toBeVisible();
      const width = await strip.evaluate((el) => ({ scroll: el.scrollWidth, client: el.clientWidth }));
      expect(width.scroll).toBeLessThanOrEqual(width.client);
      expect((await strip.getByRole("listitem").first().boundingBox())!.height).toBeLessThanOrEqual(48);
      await expectAccessible(page);
    }
  });

  const open = async (page: Page, scenario = "normal", extra = "") => {
    await page.goto(`/?dashboard=${scenario}${extra}`);
    await expect(page.getByRole("heading", { name: "My work", level: 1 })).toBeVisible();
  };
  const main = (page: Page) => page.getByRole("main");
  const status = (page: Page) => main(page).getByRole("button", { name: /^Status:/ });
  const chooseStatus = async (page: Page, choice: RegExp) => {
    await status(page).click();
    await page.getByRole("menuitemradio", { name: choice }).click();
  };
  const strip = (page: Page) => main(page).getByRole("region", { name: "Refreshing from GitHub and MemCan" });

  test("normal: shell, ranked list with every kind on, side panel, keyboard tooltips", async ({ page }) => {
    await open(page);
    await expect(
      page.getByRole("navigation", { name: "Main" }).getByRole("link", { name: /My work/ }),
    ).toHaveAttribute("aria-current", "page");
    await expect(page.locator("[data-nav-count]")).toHaveText("1");
    await expect(main(page).getByText("fix(sync): retry header fetch after peer timeout")).toBeVisible();
    // Every kind on at first: pull requests, issues and TODOs.
    await expect(main(page).getByText("Wallet balance shows a stale value after a reorg")).toBeVisible();
    await expect(main(page).getByText("Write the migration note for config v3")).toBeVisible();
    await expect(page.locator("[data-scan-info]")).toHaveText(/Full scan \d\d:\d\d/);
    // The live region says the state only, not the ticking "… ago".
    await expect(page.getByRole("status").filter({ hasText: /^Refreshed at \d\d:\d\d\.$/ })).toBeAttached();
    await expectKeyboardTooltips(page);
  });

  test("a pull request's comment counts open its Comments; without counts Watch reads them (owner 2026-11-23)", async ({
    page,
  }) => {
    await open(page);
    await main(page)
      .getByRole("link", { name: /^15 comments · 11 need you: comments of #482 / })
      .click();
    await expect(page).toHaveURL("/pr/acme/node-client/482/comments");
    await expect(page.getByRole("navigation", { name: "Queue" })).toBeVisible();
    await page.goBack();
    const menu = main(page).getByRole("button", { name: "More actions for PLAT-1203" });
    const watch = main(page).getByRole("menuitemcheckbox", { name: "Watch comments" });
    await menu.click();
    await expect(watch).toHaveAttribute("aria-checked", "false");
    await watch.click();
    await expect(
      main(page).getByRole("link", { name: /^0 comments · 0 need you: comments of #1203 / }),
    ).toBeVisible();
    await menu.click();
    await expect(watch).toHaveAttribute("aria-checked", "true");
    await watch.click();
    await menu.click();
    await expect(watch).toHaveAttribute("aria-checked", "false");
    await page.keyboard.press("Escape");
    for (const colorScheme of ["light", "dark"] as const) {
      await page.emulateMedia({ colorScheme });
      await expectAccessible(page);
    }
    await expectKeyboardTooltips(page);
  });

  test("every row shows a GitHub / MemCan icon link without hover; it opens a new tab, not the row", async ({
    page,
  }) => {
    await open(page);
    const link = main(page).getByRole("link", { name: "Open acme/platform#1203 on GitHub, new tab" });
    await expect(link).toBeVisible();
    await expect(link).toHaveAttribute("href", "https://github.com/acme/platform/pull/1203");
    await expect(link).toHaveAttribute("target", "_blank");
    const duplicate = main(page).locator(
      'a[aria-hidden="true"][href="https://github.com/acme/platform/pull/1203"]',
    );
    await expect(duplicate).toHaveAttribute("tabindex", "-1");
    await expect(duplicate.locator('[data-icon="github"]')).toBeVisible();
    const box = await link.boundingBox();
    expect(box!.width).toBeGreaterThanOrEqual(24);
    expect(box!.height).toBeGreaterThanOrEqual(24);
    await expect(
      main(page).getByRole("link", { name: "Open acme/platform#1187 on GitHub, new tab" }),
    ).toBeVisible();
    await expect(main(page).getByRole("link", { name: "Open TODO in MemCan" })).toHaveCount(2);
    await link.focus();
    await expect(page.getByRole("tooltip")).toHaveText("Open acme/platform#1203 on GitHub, new tab");
    await page.context().route("https://github.com/**", (route) => route.fulfill({ body: "GitHub" }));
    const popup = page.waitForEvent("popup");
    await link.click();
    await (await popup).close();
    expect(new URL(page.url()).pathname).toBe("/");
    await expect(main(page).getByRole("button", { name: "Details of PLAT-1203" })).toHaveAttribute(
      "aria-expanded",
      "false",
    );
  });

  test("Status lists excluded items by reason with links out and restores trigger focus", async ({
    page,
  }) => {
    await open(page);
    const toggle = status(page);
    await expect(toggle).toHaveAttribute("aria-expanded", "false");
    await toggle.focus();
    await page.keyboard.press("ArrowUp");
    const excluded = page.getByRole("menuitemradio", { name: "Excluded and snoozed · 3" });
    await expect(excluded).toBeFocused();
    await expect(excluded).toHaveAccessibleDescription(/1 excluded author · 1 outside your areas · 1 draft/);
    await page.keyboard.press("Enter");
    await expect(toggle).toBeFocused();
    await expect(toggle).toHaveAttribute("aria-expanded", "false");
    const authors = main(page).getByRole("group", { name: "Excluded authors: 1" });
    const link = authors.getByRole("link", { name: "Open WUI-330 on GitHub" });
    await expect(link).toHaveAttribute("href", "https://github.com/acme/wallet-ui/pull/330");
    await expect(link).toHaveAttribute("target", "_blank");
    await expect(authors.getByText("by @dependabot")).toBeVisible();
    await authors.getByRole("link", { name: "Excluded authors in the settings guide" }).click();
    await expect(page).toHaveURL(/\/settings\/help\?from=[^#]+#skip_authors$/);
    await page.getByRole("link", { name: "Back to My work" }).click();
    await expect(page).toHaveURL(/\/(?:\?.*)?$/);
  });

  test("snooze: a row's menu leaves the item out until Wake now in Excluded brings it back", async ({
    page,
  }) => {
    await open(page);
    const row = main(page).locator("li[data-item]").first();
    const key = await row.getAttribute("data-item");
    const id = (await row
      .getByRole("button", { name: /^More actions for / })
      .getAttribute("aria-label"))!.replace("More actions for ", "");
    await row.getByRole("button", { name: `More actions for ${id}` }).click();
    await main(page).getByRole("menuitem", { name: "Snooze…" }).click();
    const menu = main(page).getByRole("group", { name: `Snooze ${id}` });
    await menu.getByLabel("Days (1–365)").fill("400");
    await menu.getByRole("button", { name: "Snooze", exact: true }).click();
    await expect(menu.getByRole("alert")).toHaveText("Type a whole number of days from 1 to 365.");
    await menu.getByRole("button", { name: "3 days" }).click();
    await expect(main(page).locator(`li[data-item="${key}"]`)).toHaveCount(0);
    await chooseStatus(page, /^Excluded and snoozed · 4$/);
    const snoozed = main(page).getByRole("group", { name: "Snoozed: 1" });
    await expect(snoozed.getByText(/^Snoozed until /)).toBeVisible();
    await expectAccessible(page);
    await snoozed.getByRole("button", { name: `Wake now ${id}` }).click();
    await expect(snoozed).toHaveCount(0);
    await chooseStatus(page, /^All · 13$/);
    await expect(main(page).locator(`li[data-item="${key}"]`)).toHaveCount(1);
    await expect(status(page)).toHaveAccessibleName("Status: All");
  });

  test("filters combine in the URL, survive reload and clear from the chip row with Escape", async ({
    page,
  }) => {
    await open(page, "v14");
    const row = (key: string) => main(page).locator(`[data-item="${key}"]`);
    const nc = row("acme~node-client~pr~482");
    const platform = row("acme~platform~pr~1203");
    const size = nc.getByRole("button", { name: "Filter by size medium" });
    await size.focus();
    await page.keyboard.press("Enter");
    await nc.getByRole("button", { name: "Filter by label CI failing" }).click();
    await nc.getByRole("button", { name: "Filter by repo acme/node-client" }).click();
    await expect(nc).toBeVisible();
    await expect(platform).toHaveCount(0);
    await expect(main(page).getByRole("group", { name: "P0 alarm PLAT-1187" })).toBeVisible();
    expect(new URL(page.url()).searchParams.get("f")).toBe(
      "size:medium,label:ci_failing,repo:acme/node-client",
    );
    await page.reload();
    await expect(nc).toBeVisible();
    const chips = main(page).getByRole("region", { name: "Active filters" });
    const label = chips.getByRole("button", { name: "label: CI failing" });
    await label.click();
    await expect(label).toHaveAttribute("aria-pressed", "true");
    await expect(nc).toHaveCount(0);
    await page.keyboard.press("Escape");
    await expect(chips.getByText(/^Click a label/)).toBeVisible();
    await expect(platform).toBeVisible();
    expect(new URL(page.url()).searchParams.has("f")).toBe(false);
    await nc.getByRole("button", { name: /^Filter by author/ }).click({ modifiers: ["Alt"] });
    await expect(nc).toHaveCount(0);
    await expect(chips.getByRole("button", { name: /^author:/ })).toHaveAttribute("aria-pressed", "true");
  });

  test("size buttons and their row ID links have distinct 24 px targets", async ({ page }) => {
    for (const width of [390, 1440]) {
      await page.setViewportSize({ width, height: 900 });
      await open(page);
      const row = main(page).locator('[data-item="acme~wallet-ui~pr~311"]');
      const size = await row.getByRole("button", { name: "Filter by size small" }).boundingBox();
      const link = await row
        .getByRole("link", { name: "Open acme/wallet-ui#311 on GitHub, new tab", exact: true })
        .boundingBox();
      expect(size!.width).toBeGreaterThanOrEqual(24);
      expect(size!.height).toBeGreaterThanOrEqual(24);
      expect(size!.y + size!.height).toBeLessThanOrEqual(link!.y);
    }
  });

  test("filters by status and shows deterministic stack, base and App facts on a phone", async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await page.emulateMedia({ colorScheme: "dark" });
    await open(page, "v14");
    await expect(page.getByRole("button", { name: "Collapse navigation" })).toBeVisible();
    await expect(page.locator("[data-scan-info]")).toHaveText(/Live · full scan/);
    await expect(
      main(page).getByRole("button", { name: "Filter by label merges first" }),
    ).toHaveAccessibleDescription(/Bottom of a stack of 3.*#1205 and #1209/);
    await expect(main(page).getByRole("button", { name: "Filter by label release base" })).toHaveText(
      "→ release/2.4",
    );
    await chooseStatus(page, /^Not live-updated · 1$/);
    await expect(main(page).getByText("◷ not live")).toBeVisible();
    await expect(main(page).getByText("fix(sync): retry header fetch after peer timeout")).toHaveCount(0);
    const width = await main(page).evaluate((element) => element.scrollWidth - element.clientWidth);
    expect(width).toBeLessThanOrEqual(1);
    await expectAccessible(page);
    await open(page, "v14-no-app");
    await expect(page.locator("[data-scan-info]")).toHaveText(/Not live · scan/);
  });

  test("re-judges one outdated item with one click; bulk Refresh uses the C2 request body", async ({
    page,
  }) => {
    await open(page, "v14");
    await pauseClock(page);
    const nc = main(page).locator('[data-item="acme~node-client~pr~482"]');
    const mark = nc.getByRole("button", { name: /^outdated/ });
    await expect(mark).toHaveAccessibleDescription(/Facts changed/);
    await mark.click();
    await settle(page, (quick) => quick(nc.getByText("◔ judging…")).toBeVisible());
    await page.clock.runFor(5_500);
    await settle(page, (quick) => quick(mark).toHaveCount(0));
    await expect(nc.locator('[data-provenance="llm"]').first()).toHaveAccessibleDescription(/Judged on/);

    const request = page.waitForRequest(
      (r) => r.method() === "POST" && r.url().endsWith("/api/v1/dashboard/refreshes"),
    );
    await page.getByRole("button", { name: "More refresh options" }).click();
    await page.getByRole("menuitem", { name: /^Refresh and re-judge outdated/ }).click();
    expect((await request).postDataJSON()).toEqual({ rejudge_outdated: true });
    await settle(page, (quick) => quick(strip(page)).toBeVisible());
  });

  test("side panel: Ready to merge lists the user's mergeable PRs; Copy list puts linked lines on the clipboard", async ({
    page,
    context,
  }) => {
    await context.grantPermissions(["clipboard-read", "clipboard-write"]);
    await page.setViewportSize({ width: 1600, height: 900 });
    await open(page);
    const side = page.getByRole("complementary");
    const ready = side.locator("details", { has: page.getByRole("heading", { name: "Ready to merge" }) });
    await expect(ready.locator("summary")).toContainText("1 PRs");
    await expect(ready.getByRole("link", { name: "Open WUI-311 on GitHub (new tab)" })).toBeVisible();
    await expect(ready.getByText("CI failing")).toBeVisible();
    // Still ranked: the same PR is in Needs you.
    await expect(
      page
        .getByRole("region", { name: "Needs you" })
        .getByRole("link", { name: "Open acme/wallet-ui#311 on GitHub, new tab", exact: true }),
    ).toBeVisible();
    const copy = ready.getByRole("button", { name: "Copy list of PRs ready to merge" });
    const box = (await copy.boundingBox())!;
    expect(box.height).toBeGreaterThanOrEqual(24);
    await copy.focus();
    await page.keyboard.press("Enter");
    await expect(ready.getByRole("status")).toHaveText("Copied 1 line.");
    const clip = await page.evaluate(async () => {
      const [item] = await navigator.clipboard.read();
      return {
        text: await (await item!.getType("text/plain")).text(),
        html: await (await item!.getType("text/html")).text(),
      };
    });
    expect(clip.text).toBe(
      "[acme/wallet-ui#311](https://github.com/acme/wallet-ui/pull/311) feat(wallet): show pending withdrawals - small (+266 / -114)",
    );
    expect(clip.html).toContain(
      '<a href="https://github.com/acme/wallet-ui/pull/311">acme/wallet-ui#311</a>',
    );
  });

  test("Copy list over plain HTTP: without the Clipboard API a copy event still carries HTML and text", async ({
    page,
  }) => {
    // The LAN deployment is not a secure context: no navigator.clipboard there.
    await page.addInitScript(() => {
      Object.defineProperty(Navigator.prototype, "clipboard", { get: () => undefined, configurable: true });
      window.addEventListener("copy", (event) => {
        const copied = {
          text: event.clipboardData?.getData("text/plain"),
          html: event.clipboardData?.getData("text/html"),
        };
        (window as unknown as { copied: typeof copied }).copied = copied;
      });
    });
    await page.setViewportSize({ width: 1600, height: 900 });
    await open(page);
    const waiting = page
      .getByRole("complementary")
      .locator("details", { has: page.getByRole("heading", { name: "Waiting on others" }) });
    const copy = waiting.getByRole("button", { name: "Copy list of PRs waiting on others" });
    await copy.click();
    await expect(waiting.getByRole("status").filter({ hasText: /^Copied/ })).toHaveText("Copied 3 lines.");
    await expect(copy).toBeFocused();
    const copied = await page.evaluate(
      () => (window as unknown as { copied: { text: string; html: string } }).copied,
    );
    expect(copied.text.split("\n")).toHaveLength(3);
    expect(copied.text).toContain(
      "[acme/platform#1176](https://github.com/acme/platform/pull/1176) feat(sync): parallel header download - large (+3200 / -400) — waits on @jun-park, team:core for 11 d",
    );
    expect(copied.html).toContain(
      '<a href="https://github.com/acme/platform/pull/1176">acme/platform#1176</a>',
    );
  });

  test("Copy for Slack: a row's Slack icon and the list write the Slack names of Settings instead of GitHub logins", async ({
    page,
    context,
  }) => {
    await context.grantPermissions(["clipboard-read", "clipboard-write"]);
    await page.setViewportSize({ width: 1600, height: 900 });
    await open(page);
    const waiting = page
      .getByRole("complementary")
      .locator("details", { has: page.getByRole("heading", { name: "Waiting on others" }) });
    const read = () =>
      page.evaluate(async () => {
        const [item] = await navigator.clipboard.read();
        return {
          text: await (await item!.getType("text/plain")).text(),
          html: await (await item!.getType("text/html")).text(),
        };
      });
    const row = waiting.getByRole("button", { name: "Copy line of PLAT-1176 for Slack" });
    const box = (await row.boundingBox())!;
    expect(Math.min(box.width, box.height)).toBeGreaterThanOrEqual(24);
    await row.focus();
    await page.keyboard.press("Enter");
    await expect(waiting.getByText("Copied the line of PLAT-1176.")).toBeAttached();
    // fixtures/dashboard/api/settings.json maps jun-park to @jun.park; tobias-w has no Slack name.
    expect((await read()).text).toBe(
      "[acme/platform#1176](https://github.com/acme/platform/pull/1176) feat(sync): parallel header download - large (+3200 / -400) — waits on @jun.park, team:core for 11 d",
    );
    const list = waiting.getByRole("button", { name: "Copy for Slack: list of PRs waiting on others" });
    await expect(list).toHaveText("Copy for Slack");
    await list.click();
    await expect(waiting.getByRole("status").filter({ hasText: /^Copied \d+ lines/ })).toHaveText(
      "Copied 3 lines.",
    );
    const clip = await read();
    expect(clip.text.split("\n")).toHaveLength(3);
    expect(clip.text).toContain("waits on @jun.park, team:core for 11 d");
    expect(clip.text).toContain("waits on @tobias-w for");
    expect(clip.html).toContain("waits on @jun.park, team:core for 11 d");
    // The panel itself keeps the GitHub logins; the plain copy too.
    await expect(waiting.getByText(/^waits on @jun-park, team:core for/)).toBeVisible();
    await waiting.getByRole("button", { name: "Copy line of PLAT-1176", exact: true }).click();
    // The clipboard may still be written when first read ("Clipboard data has changed"): read until it holds the line.
    await expect(async () =>
      expect((await read()).text).toContain("waits on @jun-park, team:core for 11 d"),
    ).toPass();
  });

  test("kind toggles combine by keyboard, the last one on stays on, and the choice is remembered", async ({
    page,
  }) => {
    await open(page);
    const kinds = main(page).getByRole("group", { name: "Kinds shown" });
    const issue = main(page).getByText("Wallet balance shows a stale value after a reorg");
    const todo = main(page).getByText("Write the migration note for config v3");
    await kinds.getByRole("button", { name: /^Issues/ }).focus();
    await page.keyboard.press("Enter");
    await expect(issue).toHaveCount(0);
    await kinds.getByRole("button", { name: /^PRs/ }).focus();
    await page.keyboard.press("Space");
    await expect(todo).toBeVisible();
    await expect(main(page).getByText("fix(sync): retry header fetch after peer timeout")).toHaveCount(0);
    const todos = kinds.getByRole("button", { name: /^TODOs/ });
    await expect(todos).toHaveAttribute("aria-disabled", "true");
    await todos.focus();
    await page.keyboard.press("Enter");
    await expect(todos).toHaveAttribute("aria-pressed", "true");
    await page.reload();
    await expect(kinds.getByRole("button", { name: /^TODOs/ })).toHaveAttribute("aria-pressed", "true");
    await expect(kinds.getByRole("button", { name: /^PRs/ })).toHaveAttribute("aria-pressed", "false");
    await expect(todo).toBeVisible();
  });

  test("refreshing: scan Details and Cancel keep the earlier snapshot", async ({ page }) => {
    await open(page, "refreshing");
    await expect(strip(page)).toContainText("PR details 23 / 41");
    await expect(strip(page).getByRole("list")).toHaveCount(0);
    await strip(page).getByRole("button", { name: "Details" }).click();
    await expect(strip(page).getByRole("listitem")).toHaveCount(7);
    const progress = strip(page).getByRole("progressbar", { name: "Scan progress" });
    await expect(progress).toHaveAttribute("aria-valuenow", "23");
    await expect(progress).toHaveAttribute("aria-valuemax", "41");
    await expect(page.getByRole("button", { name: /Refreshing…/ })).toHaveAttribute("aria-disabled", "true");
    await page.getByRole("button", { name: "More refresh options" }).click();
    await page.getByRole("menuitem", { name: "Cancel scan" }).click();
    await expect(strip(page)).toHaveCount(0);
    await expect(main(page).getByText("fix(sync): retry header fetch after peer timeout")).toBeVisible();
  });

  test("Refresh: scan, provisional ranking, then Ranking finished (mock clock)", async ({ page }) => {
    await open(page);
    await pauseClock(page);
    await page.getByRole("button", { name: "Refresh", exact: true }).click();
    await settle(page, (quick) => quick(strip(page)).toBeVisible());
    await page.clock.runFor(5.5 * SCRIPT_STEP_MS);
    await settle(page, async (quick) => {
      await quick(strip(page)).toHaveCount(0);
      await quick(main(page).getByRole("heading", { name: /^Not judged yet/ })).toBeVisible();
    });
    await page.clock.runFor(2 * SCRIPT_STEP_MS);
    await settle(page, (quick) => quick(page.getByText("Ranking finished: 13 items judged.")).toBeVisible());
  });

  test("provisional and failed ranking show row marks without a ranking banner", async ({ page }) => {
    for (const scenario of ["provisional", "waiting", "ranking-failed"]) {
      await open(page, scenario);
      await expect(main(page).getByRole("heading", { name: /^Not judged yet/ })).toBeVisible();
      await expect(
        main(page)
          .getByRole("button", { name: /^outdated/ })
          .first(),
      ).toBeVisible();
      await expect(page.getByText(/Ranking with the LLM:/)).toHaveCount(0);
    }
  });

  test("ranking failure keeps the reason and can be retried without starting a scan", async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await open(page, "ranking-failed");
    await pauseClock(page);
    const info = page.locator("[data-scan-info]");
    await expect(info).toContainText("Ranking failed");
    await expect(info).toHaveAccessibleDescription(/The LLM stopped answering/);
    await status(page).click();
    await expect(
      main(page).getByRole("menuitemradio", { name: /^Not judged yet/ }),
    ).toHaveAccessibleDescription(/Ranking failed/);
    await page.keyboard.press("Escape");
    await info.focus();
    await page.keyboard.press("Tab");
    const retry = page.getByRole("button", { name: "Retry ranking", exact: true });
    await expect(retry).toBeFocused();
    expect((await retry.boundingBox())!.height).toBeGreaterThanOrEqual(24);
    await page.keyboard.press("Enter");
    await settle(page, (quick) => quick(info).not.toContainText("Ranking failed"));
    await expect(info).toBeFocused();
    await expect(strip(page)).toHaveCount(0);
  });

  test("legacy daily limit caps the unjudged row and the older prompt stays in the scan status", async ({
    page,
  }) => {
    await open(page, "ranking-daily-cap");
    const info = page.locator("[data-scan-info]");
    await expect(info).toContainText("Daily limit reached · resets at 00:00");
    await expect(info).toHaveAccessibleDescription(/2 items were judged with an older ranking prompt/);
    await status(page).click();
    await expect(
      main(page).getByRole("menuitemradio", { name: /^Outdated judgement/ }),
    ).toHaveAccessibleDescription("Judgement inputs changed after the LLM judged these items.");
    const unjudged = main(page).getByRole("menuitemradio", { name: /^Not judged yet/ });
    await expect(unjudged).toHaveAccessibleDescription(/00:00.*Europe\/Warsaw/);
    await unjudged.click();
    await expect(main(page).getByText("◌ not judged · daily limit")).toBeVisible();
  });

  test("an unknown stored zone leaves My work usable and names the browser fallback", async ({ page }) => {
    await open(page, "unknown-time-zone");
    await expect(main(page).getByText(/Your browser does not recognize Etc\/Unknown/)).toBeVisible();
    await expect(
      main(page).getByRole("heading", { name: "Closed − opened, today" }),
    ).toHaveAccessibleDescription(/browser’s time zone/);
  });

  test("MemCan unreachable: each kept TODO row says it was not refreshed, with a tooltip", async ({
    page,
  }) => {
    await open(page, "memcan-down");
    const todoRows = main(page).locator("li[data-item]").filter({ hasText: "MemCan" });
    await expect(todoRows.first()).toBeVisible();
    const count = await todoRows.count();
    expect(count).toBeGreaterThan(0);
    for (let i = 0; i < count; i++) {
      await expect(todoRows.nth(i)).toContainText(/not refreshed \(MemCan unreachable at \d\d:\d\d\)/);
    }
    const mark = todoRows.first().getByText(/◷ not refreshed/);
    await mark.focus();
    await expect(page.getByRole("tooltip")).toContainText("MemCan could not be reached in this scan");
  });

  test("error: the rate limit stops the refresh; the 08:12 snapshot stays and Refresh waits", async ({
    page,
  }) => {
    await open(page, "error");
    await expect(
      page.getByRole("alert").filter({ hasText: "GitHub rate limit reached; the refresh stopped." }),
    ).toBeVisible();
    await expect(page.getByRole("button", { name: /^Retry at / }).first()).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    await expect(main(page).getByText(/^Previous snapshot · \d\d:\d\d$/)).toBeVisible();
    await expect(main(page).getByText("It is replaced when a scan finishes.")).toBeVisible();
  });

  test("empty: nothing needs you; your PRs waiting on others stay in the side panel", async ({ page }) => {
    await open(page, "empty");
    await expect(main(page).getByText("Nothing needs you right now.")).toBeVisible();
    await expect(page.locator("[data-nav-count]")).toHaveText("0");
  });

  test("first run: nothing scanned yet, then Refresh starts the first scan", async ({ page }) => {
    await open(page, "first-run");
    await expect(page.getByText("Nothing scanned yet.")).toBeVisible();
    await page.getByRole("button", { name: "Refresh", exact: true }).click();
    await expect(strip(page)).toBeVisible();
    await expect(main(page).getByText("fix(sync): retry header fetch after peer timeout")).toBeVisible({
      timeout: 10_000,
    });
  });

  test.describe("first visit", () => {
    test.use({ timezoneId: "Europe/Lisbon" });

    test("stores the browser's time zone once (R6b); the schedule note goes", async ({ page }) => {
      const saved = page.waitForRequest(
        (r) => r.method() === "PUT" && r.url().endsWith("/api/v1/settings/dashboard/time-zone"),
      );
      await open(page, "no-time-zone");
      expect((await saved).postDataJSON()).toEqual({ time_zone: "Europe/Lisbon" });
      await expect(page.getByText(/starts once your time zone is known/)).toHaveCount(0);
    });

    test("a refused time zone: the note points to Settings instead", async ({ page }) => {
      await open(page, "no-time-zone-refused");
      await expect(
        page.getByText("Your browser's time zone could not be saved; set it in GitHub sync."),
      ).toBeVisible();
      await expect(page.getByText(/This page saves your browser's time zone/)).toHaveCount(0);
    });
  });

  test("1600×900, side panel open: headline beside its chart, first ranked row starts above the fold", async ({
    page,
  }) => {
    await page.setViewportSize({ width: 1600, height: 900 });
    await open(page);
    await expect(page.getByRole("separator", { name: "Resize the side panel" })).toBeVisible();
    const hero = (await page.locator("[data-headline-value]").boundingBox())!;
    const chart = (await page.getByRole("img", { name: /^Net closed minus opened/ }).boundingBox())!;
    expect(chart.x).toBeGreaterThan(hero.x + hero.width);
    await expect
      .poll(async () => {
        const left = await page.locator("[data-headline-figures]").boundingBox();
        const right = await page.locator("[data-headline-chart]").boundingBox();
        return Math.round(right!.height - left!.height);
      })
      .toBe(8);
    const first = (await page.locator("li[data-item]").first().boundingBox())!;
    expect(first.y).toBeLessThan(900);
  });

  test("a chart bar lists its pull requests under the chart; again, Hide or another period closes it", async ({
    page,
  }) => {
    await open(page);
    // Mon 16 Nov: the third bar from the right of the 20 working days.
    const { chart, at } = await netChartBar(page, 17);
    const list = page.getByRole("region", { name: "Pull requests of Mon 16 Nov" });
    await chart.hover(at);
    await expect(page.locator("[data-chart-tip]")).toContainText(
      "Click or press Enter to list its pull requests",
    );
    await chart.click(at);
    await expect(list.getByRole("group", { name: "Opened (2)" })).toBeVisible();
    const closed = list.getByRole("group", { name: "Closed (3)" });
    await expect(closed.getByRole("listitem").filter({ hasText: "acme/node-client#1114" })).toContainText(
      "closed without merge",
    );
    await expect(closed.getByRole("link", { name: /^acme\/platform#1116/ })).toHaveAttribute(
      "href",
      "https://github.com/acme/platform/pull/1116",
    );
    await chart.click(at);
    await expect(list).toHaveCount(0);
    // By keyboard: read a bar, Enter lists it, Hide returns to the chart. Focus starts on the last bar.
    await page.mouse.move(0, 0);
    await chart.blur();
    await chart.focus();
    for (let i = 0; i < 2; i++) await page.keyboard.press("ArrowLeft");
    await page.keyboard.press("Enter");
    await expect(list).toBeVisible();
    await list.getByRole("button", { name: "Hide pull requests of Mon 16 Nov" }).focus();
    await page.keyboard.press("Enter");
    await expect(list).toHaveCount(0);
    await expect(chart).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(list).toBeVisible();
    await page.getByRole("button", { name: "Weeks" }).click();
    await expect(page.getByRole("region", { name: /^Pull requests of/ })).toHaveCount(0);
  });

  test("signed out: the sign-in card", async ({ page }) => {
    await page.goto("/?scenario=signed-out");
    await expect(page.getByRole("link", { name: "Sign in with GitHub" })).toHaveAttribute(
      "href",
      "/auth/github/login",
    );
  });

  for (const colorScheme of ["light", "dark"] as const) {
    test(`no serious axe violations in every mockup state, ${colorScheme}`, async ({ page }) => {
      // One axe pass per mockup state; CI runners need more than the default 30 s.
      test.setTimeout(120_000);
      await page.emulateMedia({ colorScheme });
      for (const scenario of [
        "normal",
        "refreshing",
        "provisional",
        "waiting",
        "ranking-failed",
        "empty",
        "error",
        "first-run",
        "no-time-zone",
        "no-time-zone-refused",
      ]) {
        await test.step(scenario, async () => {
          await open(page, scenario);
          await expectAccessible(page);
        });
      }
    });

    test(`no serious axe violations at 1600 px: side panel open, a row expanded, chart table and bar list, ${colorScheme}`, async ({
      page,
    }) => {
      await page.setViewportSize({ width: 1600, height: 900 });
      await page.emulateMedia({ colorScheme });
      await open(page);
      await expect(page.getByRole("separator", { name: "Resize the side panel" })).toBeVisible();
      await page.locator("li[data-item] button[aria-expanded]").first().click();
      const closed = page.locator("aside details:not([open]) > summary");
      while ((await closed.count()) > 0) await closed.first().click();
      await chooseStatus(page, /^Excluded and snoozed · 3$/);
      await expect(page.getByRole("group", { name: "Drafts: 1" })).toBeVisible();
      await page.getByRole("button", { name: "Show as table" }).click();
      await expect(page.getByRole("region", { name: "Net per day as a table" })).toBeVisible();
      const bar = await netChartBar(page, 17);
      await bar.chart.click(bar.at);
      await expect(page.getByRole("region", { name: "Pull requests of Mon 16 Nov" })).toBeVisible();
      await expectAccessible(page);
    });
  }
});

test.describe("review start from a PR row (msw)", () => {
  const row = (page: Page) =>
    page.getByRole("listitem").filter({ hasText: "feat(rpc): retry with backoff on timeout" });

  test("new: Review opens the review start page with the PR, checked on arrival: Start analysis", async ({
    page,
  }) => {
    await page.goto("/?dashboard=review-new");
    await row(page)
      .getByRole("link", { name: /^Review pull request / })
      .click();
    await expect(page).toHaveURL(/\/review\?pr=/);
    await expect(page.getByLabel(/pull request URL/i)).toHaveValue(PR_URL);
    await expect(page.getByRole("button", { name: "Start analysis", exact: true })).toBeVisible();
  });

  test("found: the row opens the review of the current head", async ({ page }) => {
    await page.goto("/?dashboard=review-found");
    await row(page)
      .getByRole("link", { name: /^Open review of / })
      .click();
    await expect(page).toHaveURL(new RegExp(`/analyses/${ANALYSIS_ID}`));
  });

  test("moved: the review start page says the PR has moved", async ({ page }) => {
    await page.goto("/?dashboard=review-moved");
    await row(page)
      .getByRole("link", { name: /^Open review of / })
      .click();
    await expect(page.getByLabel(/pull request URL/i)).toHaveValue(PR_URL);
    await expect(page.getByText("The PR has moved.")).toBeVisible();
  });
});

test("failed mention hide keeps the row and count and shows a retry message", async ({ page }) => {
  await page.goto("/?dashboard=mention-hide-fails");
  const strip = page.getByRole("region", { name: "Mentions" });
  await strip.getByRole("button", { name: "Mentions (4)" }).click();
  await strip
    .getByRole("button", { name: /Hide mention/ })
    .first()
    .click();
  await expect(page.getByText(/Could not hide this mention. Try again./)).toBeVisible();
  await expect(strip.getByRole("link")).toHaveCount(3);
  await expect(page.locator("[data-nav-count]")).toHaveText("4");
  await page.getByRole("button", { name: "Dismiss", exact: true }).click();
  await expect(page.getByText(/Could not hide this mention. Try again./)).toHaveCount(0);
});

test("hiding a vanished mention removes its row and updates the count without an error", async ({ page }) => {
  await page.goto("/?dashboard=mention-hide-gone");
  const strip = page.getByRole("region", { name: "Mentions" });
  await strip.getByRole("button", { name: "Mentions (4)" }).click();
  const first = strip.getByRole("button", { name: /Hide mention/ }).first();
  const href = await strip.getByRole("link").first().getAttribute("href");
  await first.click();
  await expect(page.locator("[data-nav-count]")).toHaveText("3");
  await expect(strip.locator(`a[href="${href}"]`)).toHaveCount(0);
  await expect(page.getByText(/Could not hide this mention. Try again./)).toHaveCount(0);
});

test("structured mention coverage stays in the collapsed summary and out of scan notes", async ({ page }) => {
  await page.setViewportSize({ width: 1600, height: 900 });
  await page.goto("/?dashboard=mention-coverage");
  const strip = page.getByRole("region", { name: "Mentions" });
  await expect(strip).toContainText("Mentions not checked on 5 items this refresh.");
  await expect(strip).toContainText("Mention search for @alice-helper is unavailable; coverage is limited.");
  expect((await strip.boundingBox())!.height).toBeLessThanOrEqual(28);
  const heading = (await strip.getByRole("button", { name: "Mentions (1)" }).boundingBox())!;
  const coverage = (await strip.locator("span[title]").boundingBox())!;
  expect(coverage.x).toBeGreaterThanOrEqual(heading.x + heading.width);
  const first = (await page.locator("li[data-item]").first().boundingBox())!;
  expect(first.y).toBeLessThan(900);
  await strip.getByRole("button", { name: "Mentions (1)" }).focus();
  await expect(page.getByRole("tooltip")).toContainText("@alice-helper");
  await expect(page.getByRole("tooltip")).toContainText("@slow");
  await strip.getByRole("button", { name: "Mentions (1)" }).click();
  await expect(strip.getByText(/^Mention search for @alice-helper/)).toBeVisible();
  await expect(page.locator("aside")).not.toContainText("Mentions not checked");
});
