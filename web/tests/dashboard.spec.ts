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
    await expect(page.locator("[data-scan-info]")).toHaveText(/Refreshed manually at/);
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
    await main(page)
      .getByRole("button", { name: /^Watch #1203 / })
      .click();
    await expect(
      main(page).getByRole("link", { name: /^0 comments · 0 need you: comments of #1203 / }),
    ).toBeVisible();
    await expect(main(page).getByRole("button", { name: /^Watch #1203 / })).toHaveCount(0);
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
    const link = main(page).getByRole("link", { name: "Open PLAT-1203 on GitHub" });
    await expect(link).toBeVisible();
    await expect(link).toHaveAttribute("href", "https://github.com/acme/platform/pull/1203");
    await expect(link).toHaveAttribute("target", "_blank");
    const box = await link.boundingBox();
    expect(box!.width).toBeGreaterThanOrEqual(24);
    expect(box!.height).toBeGreaterThanOrEqual(24);
    await expect(main(page).getByRole("link", { name: "Open PLAT-1187 on GitHub" })).toBeVisible();
    await expect(main(page).getByRole("link", { name: "Open TODO in MemCan" })).toHaveCount(2);
    await link.focus();
    await expect(page.getByRole("tooltip")).toHaveText("Open PLAT-1203 on GitHub (new tab)");
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

  test("excluded items: a quiet line under Needs you; by keyboard it lists them by reason with links out", async ({
    page,
  }) => {
    await open(page);
    const toggle = main(page).getByRole("button", { name: /^Excluded: 3/ });
    await expect(toggle).toHaveText(/Excluded: 3 — 1 excluded author · 1 outside your areas · 1 draft/);
    await expect(toggle).toHaveAttribute("aria-expanded", "false");
    await toggle.focus();
    await page.keyboard.press("Enter");
    await expect(toggle).toHaveAttribute("aria-expanded", "true");
    const authors = main(page).getByRole("group", { name: "Excluded authors: 1" });
    const link = authors.getByRole("link", { name: "Open WUI-330 on GitHub" });
    await expect(link).toHaveAttribute("href", "https://github.com/acme/wallet-ui/pull/330");
    await expect(link).toHaveAttribute("target", "_blank");
    await expect(authors.getByText("by @dependabot")).toBeVisible();
    await authors.getByRole("link", { name: "Excluded authors in the settings guide" }).click();
    await expect(page).toHaveURL(/\/settings\/help#skip_authors$/);
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
    const menu = main(page).getByRole("group", { name: `Snooze ${id}` });
    await menu.getByLabel("Days (1–365)").fill("400");
    await menu.getByRole("button", { name: "Snooze", exact: true }).click();
    await expect(menu.getByRole("alert")).toHaveText("Type a whole number of days from 1 to 365.");
    await menu.getByRole("button", { name: "3 days" }).click();
    await expect(main(page).locator(`li[data-item="${key}"]`)).toHaveCount(0);
    const toggle = main(page).getByRole("button", { name: /^Excluded: 4/ });
    await expect(toggle).toHaveText(/1 snoozed/);
    await toggle.click();
    const snoozed = main(page).getByRole("group", { name: "Snoozed: 1" });
    await expect(snoozed.getByText(/^Snoozed until /)).toBeVisible();
    await expectAccessible(page);
    await snoozed.getByRole("button", { name: `Wake now ${id}` }).click();
    await expect(main(page).locator(`li[data-item="${key}"]`)).toHaveCount(1);
    await expect(main(page).getByRole("button", { name: /^Excluded: 3/ })).toBeVisible();
  });

  test("size pills on PR rows and Waiting on others; Sizes shown filters pull requests only", async ({
    page,
  }) => {
    await page.setViewportSize({ width: 1600, height: 900 });
    await open(page);
    const pill = main(page).getByRole("img", { name: "Change size: medium, 500 changed lines" });
    await expect(pill).toBeVisible();
    await pill.focus();
    await expect(page.getByRole("tooltip")).toContainText("500 changed lines (+350 −150); tiny < 150");
    await expect(page.getByRole("img", { name: "Change size: large, 3600 changed lines" })).toBeVisible();
    const sizes = main(page).getByRole("group", { name: "Sizes shown" });
    const medium = sizes.getByRole("button", { name: /medium 4/ });
    await medium.focus();
    await page.keyboard.press("Enter");
    await expect(medium).toHaveAttribute("aria-pressed", "false");
    await expect(main(page).getByText("fix(sync): retry header fetch after peer timeout")).toHaveCount(0);
    await expect(main(page).getByText("Wallet balance shows a stale value after a reorg")).toBeVisible();
    await page.reload();
    await expect(
      main(page)
        .getByRole("group", { name: "Sizes shown" })
        .getByRole("button", { name: /medium 4/ }),
    ).toHaveAttribute("aria-pressed", "false");
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
      page.getByRole("region", { name: "Needs you" }).getByRole("link", { name: "Open WUI-311 on GitHub" }),
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

  test("refreshing: the scan's progress and Cancel scan keep the earlier snapshot", async ({ page }) => {
    await open(page, "refreshing");
    await expect(page.getByRole("progressbar", { name: "Scan progress" })).toHaveAttribute(
      "aria-valuenow",
      "45",
    );
    await expect(page.getByRole("button", { name: /Refreshing/ })).toHaveAttribute("aria-disabled", "true");
    await page.getByRole("button", { name: "Cancel scan" }).click();
    await expect(page.getByRole("progressbar", { name: "Scan progress" })).toHaveCount(0);
    await expect(main(page).getByText("fix(sync): retry header fetch after peer timeout")).toBeVisible();
  });

  test("Refresh: scan, provisional ranking, then Ranking finished (mock clock)", async ({ page }) => {
    await open(page);
    await pauseClock(page);
    await page.getByRole("button", { name: "Refresh" }).click();
    await settle(page, (quick) =>
      quick(page.getByRole("progressbar", { name: "Scan progress" })).toBeVisible(),
    );
    // Into the scripted ranking (steps 5–6); time stands still there until the next jump.
    await page.clock.runFor(5.5 * SCRIPT_STEP_MS);
    await settle(page, async (quick) => {
      await quick(page.getByText(/Ranking with the LLM: \d+ of 13 items judged\./)).toBeVisible();
      await quick(main(page).getByText("Not judged yet", { exact: true })).toBeVisible();
    });
    await page.clock.runFor(2 * SCRIPT_STEP_MS);
    await settle(page, async (quick) => {
      await quick(page.getByText("Ranking finished: 13 items judged.")).toBeVisible();
      await quick(page.getByText(/Ranking with the LLM/)).toHaveCount(0);
    });
  });

  test("provisional and waiting for the review analysis", async ({ page }) => {
    await open(page, "provisional");
    await expect(page.getByText("Ranking with the LLM: 10 of 13 items judged.")).toBeVisible();
    await expect(main(page).getByText("fix(ui): restore the focus ring on tabs")).toBeVisible();
    await open(page, "waiting");
    await expect(page.getByText("Waiting for the review analysis to finish.")).toBeVisible();
  });

  test("a failed ranking keeps the estimates; Retry ranking ranks again (mock clock)", async ({ page }) => {
    await open(page, "ranking-failed");
    await expect(
      page.getByText("LLM ranking failed: The LLM stopped answering. Provisional scores stay approximate."),
    ).toBeVisible();
    await pauseClock(page);
    await page.getByRole("button", { name: "Retry ranking" }).click();
    await settle(page, (quick) => quick(page.getByText(/Ranking with the LLM/)).toBeVisible());
    await page.clock.runFor(2.5 * SCRIPT_STEP_MS);
    await settle(page, (quick) => quick(page.getByText("Ranking finished: 13 items judged.")).toBeVisible());
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
    const mark = todoRows.first().getByText(/^~ not refreshed/);
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
    await page.getByRole("button", { name: "Refresh" }).click();
    await expect(page.getByRole("progressbar", { name: "Scan progress" })).toBeVisible();
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
        page.getByText("Your browser's time zone could not be saved; set it in Settings."),
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
      await page.getByRole("button", { name: /^Excluded: 3/ }).click();
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
