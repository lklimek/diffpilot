import { expect, test } from "@playwright/test";
import { ANALYSIS_ID, expectTrustWording, openTour } from "./support";

const heading = (page: import("@playwright/test").Page) =>
  page.locator("main h2:not([data-column-title])").first();

test("PR URL → PR context → flow 1 introduction → step 1 via j", async ({ page }) => {
  await page.goto("/");
  await page.getByLabel("GitHub pull request URL").fill("https://github.com/acme/node-client/pull/482");
  await page.getByRole("button", { name: "Analyze" }).click();
  await expect(page).toHaveURL(new RegExp(`/analyses/${ANALYSIS_ID}/progress$`));
  await page.getByRole("link", { name: "Open review" }).click();
  await expect(page).toHaveURL(new RegExp(`/analyses/${ANALYSIS_ID}$`));
  await expect(heading(page)).toHaveText("PR context: acme/node-client #482");

  await page.keyboard.press("j");
  await expect(heading(page)).toHaveText("1 Send an RPC request with retry");
  await expect(
    page.getByRole("navigation").getByRole("button", { name: "Flow 1: Send an RPC request with retry" }),
  ).toHaveAttribute("aria-current", "step");

  await page.keyboard.press("j");
  await expect(heading(page)).toHaveText("1 Trigger: callers of send");
  const code = page.getByRole("region", { name: "src/mempool/mod.rs, lines 126 to 132" });
  await expect(code).toContainText("pub async fn broadcast_tx");
  await expect(
    page.getByRole("navigation").getByRole("button", { name: "Step 1: Trigger: callers of send" }),
  ).toHaveAttribute("aria-current", "step");

  await page.keyboard.press("k");
  await expect(heading(page)).toHaveText("1 Send an RPC request with retry");
});

test("Previous / Next walk the sequence; after a flow's last step comes the next flow", async ({ page }) => {
  await openTour(page);
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Flow 1: Send an RPC request with retry" })
    .click();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /^Step 10:/ })
    .click();
  await expect(page.getByRole("button", { name: /^Next: Flow 2:/ })).toBeVisible();
  await page.getByRole("button", { name: /^Next: Flow 2:/ }).click();
  await expect(heading(page)).toContainText("Default timeout");
  await page.getByRole("button", { name: /^← Previous: Step 10:/ }).click();
  await expect(heading(page)).toContainText("Retry policy");
});

test("j / k are ignored while typing in a field", async ({ page }) => {
  await page.goto("/");
  const field = page.getByLabel("GitHub pull request URL");
  await field.fill("j");
  await field.press("j");
  await expect(field).toHaveValue("jj");
});

test("step block: gutter marks, one highlight with was → is, collapsed assessment", async ({ page }) => {
  await openTour(page);
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Flow 1: Send an RPC request with retry" })
    .click();
  await page.getByRole("navigation").getByRole("button", { name: "Step 5: Retry allowed?" }).click();
  const mark = page.locator("[role=mark]");
  await expect(mark).toHaveCount(1);
  await mark.hover();
  await expect(page.getByRole("tooltip").filter({ visible: true })).toContainText("Was: ");
  const summary = page.getByText("Assessment (LLM)");
  await expect(page.locator("details")).not.toHaveAttribute("open");
  await summary.click();
  await expect(page.locator("details")).toHaveAttribute("open");
});

test("resume opens the last viewed block", async ({ page }) => {
  await page.goto(`/analyses/${ANALYSIS_ID}?state=resume`);
  await expect(heading(page)).toHaveText("5 Retry allowed?");
});

test("an unknown analysis shows a plain error", async ({ page }) => {
  await page.goto("/analyses/00000000-0000-0000-0000-000000000000");
  await expect(page.getByRole("alert")).toHaveText("This analysis does not exist.");
});

for (const state of ["default", "wrong", "missed", "noq", "noprofile", "resume"]) {
  test(`wording rules on the rendered blocks in state ${state}`, async ({ page }) => {
    await openTour(page, state);
    await page.getByRole("button", { name: "PR context" }).click();
    await expectTrustWording(page);
    for (const flow of [1, 2, 3, 4, 5]) {
      await page.getByRole("button", { name: new RegExp(`^Flow ${flow}:`) }).click();
      await expectTrustWording(page);
      const steps = page.getByRole("navigation").getByRole("button", { name: /^Step / });
      const count = await steps.count();
      for (let i = 0; i < count; i += 1) {
        await steps.nth(i).click();
        await expect(page.getByRole("status")).toHaveCount(0);
        await expectTrustWording(page);
      }
    }
  });
}

test("noq: the step says the LLM raised no questions", async ({ page }) => {
  await openTour(page, "noq");
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Flow 1: Send an RPC request with retry" })
    .click();
  await page.getByRole("navigation").getByRole("button", { name: "Step 5: Retry allowed?" }).click();
  const inspector = page.getByRole("complementary", { name: "Inspector" });
  await inspector.getByRole("button", { name: "Questions to check" }).click();
  await expect(
    inspector.getByText("The LLM raised no questions. That is not evidence of no defects."),
  ).toBeVisible();
});

test("noprofile: the PR context states the missing project profile", async ({ page }) => {
  await openTour(page, "noprofile");
  await expect(
    page.getByRole("main").getByText("No project profile — ratings are component-level"),
  ).toBeVisible();
});

test("agent_output_xss_renders_as_text", async ({ page }) => {
  const markup = '<img src=x onerror="window.__xss=1"><script>window.__xss=2</script>';
  const dialogs: string[] = [];
  page.on("dialog", (dialog) => {
    dialogs.push(dialog.message());
    void dialog.dismiss();
  });
  /** Nothing the agent wrote became markup, a handler or a script link anywhere in the app. */
  const expectInert = async () => {
    const root = page.locator("#root");
    await expect(root.locator("img, script, iframe, object, embed, foreignObject")).toHaveCount(0);
    expect(
      await root.evaluate((el) =>
        [...el.querySelectorAll("*")].flatMap((node) =>
          [...node.attributes]
            .filter((a) => a.name.startsWith("on") || /^\s*javascript:/i.test(a.value))
            .map((a) => `${node.tagName} ${a.name}`),
        ),
      ),
    ).toEqual([]);
    for (const href of await root
      .locator("a[href]")
      .evaluateAll((as) => as.map((a) => a.getAttribute("href")))) {
      expect(href).toMatch(/^(https:\/\/|\/|#)/);
    }
    expect(await page.evaluate(() => (window as { __xss?: number }).__xss)).toBeUndefined();
  };
  await page.goto(`/analyses/${ANALYSIS_ID}?scenario=xss`);
  const main = page.getByRole("main");
  const nav = page.getByRole("navigation");
  const inspector = page.getByRole("complementary", { name: "Inspector" });
  await expect(main.getByText(markup).first()).toBeVisible();
  await expectInert();

  // Flow introduction and the diagram (node and branch labels).
  await page.keyboard.press("j");
  await expect(heading(page)).toContainText(`${markup} retry`);
  await main.getByRole("button", { name: /Show flow diagram/ }).click();
  const diagram = main.getByRole("region", { name: "Flow diagram" });
  await expect(diagram.locator("svg").first()).toBeVisible();
  await expect(diagram.getByRole("button", { name: /^Step 1, .*<img src=x/ })).toBeVisible();
  await expectInert();

  // Step 1: title (heading and menu aria-label), why, inspector, the pinned LLM answer.
  await page.keyboard.press("j");
  await expect(heading(page)).toContainText(`${markup} trigger`);
  await expect(nav.getByRole("button", { name: `Step 1: ${markup} trigger` })).toBeVisible();
  await expect(inspector.getByText(`${markup} answer`)).toBeVisible();
  await expectInert();

  // Step 5: highlight tooltip (was → is) and the thread marker's title.
  await nav.getByRole("button", { name: /^Step 5:/ }).click();
  const mark = page.locator("[role=mark]").first();
  await mark.hover();
  await expect(page.getByRole("tooltip").filter({ visible: true })).toContainText(`Was: ${markup}`);
  await expect(page.locator('[data-mark="thread"]').first()).toHaveAttribute(
    "title",
    new RegExp(`by .*: ${markup.slice(0, 10)}`),
  );
  await expectInert();
  expect(dialogs).toEqual([]);
});

test("columns: hide, full screen and widths are remembered; a click anywhere on a strip restores", async ({
  page,
}) => {
  await openTour(page);
  const separator = page.getByRole("separator", { name: /Resize the Flows column/ });
  const before = Number(await separator.getAttribute("aria-valuenow"));
  await separator.focus();
  await page.keyboard.press("ArrowRight");
  await page.keyboard.press("ArrowRight");
  await expect(separator).toHaveAttribute("aria-valuenow", String(before + 48));
  await page.getByRole("button", { name: "Hide Inspector column" }).click();
  await page.reload();
  const strip = page.getByRole("button", { name: "Show Inspector column" });
  await expect(strip).toBeVisible();
  await expect(page.getByRole("separator", { name: /Resize the Flows column/ })).toHaveAttribute(
    "aria-valuenow",
    String(before + 48),
  );
  const box = (await strip.boundingBox())!;
  await page.mouse.click(box.x + box.width / 2, box.y + box.height - 20);
  await expect(page.getByRole("group", { name: "Ask, feedback and comment" })).toBeVisible();
  await page.getByRole("button", { name: "Full screen Review column" }).click();
  await expect(page.getByRole("button", { name: "Show Flows column" })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("button", { name: "Hide Flows column" })).toBeVisible();
});

test("reload keeps the position and the ticks", async ({ page }) => {
  await openTour(page);
  await page.keyboard.press("j");
  await page.keyboard.press("j");
  await page.keyboard.press("x");
  await expect(heading(page)).toHaveText("1 Trigger: callers of send");
  await page.reload();
  await expect(heading(page)).toHaveText("1 Trigger: callers of send");
  await expect(
    page.getByRole("navigation").getByRole("button", { name: "Step 1: Trigger: callers of send, reviewed" }),
  ).toBeVisible();
});

test("keyboard map: x ticks, d old code, c comment, a ask, ? help, Esc closes", async ({ page }) => {
  await openTour(page);
  const banner = page.getByRole("banner").first();
  const main = page.getByRole("main");
  const inspector = page.getByRole("complementary", { name: "Inspector" });
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /^Flow 2:/ })
    .click();
  await page.keyboard.press("j");
  const code = main.getByRole("region", { name: /^src\/config\.rs, lines 12 to 26/ });
  await expect(code).toBeVisible();
  await page.keyboard.press("x");
  await page.keyboard.press("d");
  await expect(main.getByRole("region", { name: "Classic diff of src/config.rs" })).toBeVisible();
  await page.keyboard.press("d");
  await expect(code).toBeVisible();
  await page.keyboard.press("c");
  await expect(inspector.getByRole("textbox", { name: /^Comment on src\/config\.rs:/ })).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(inspector.getByRole("textbox", { name: /^Comment on / })).toHaveCount(0);
  await page.keyboard.press("a");
  const ask = inspector.getByRole("textbox", { name: /^Ask the LLM about / });
  await expect(ask).toBeFocused();
  await ask.blur();
  await page.keyboard.press("?");
  await expect(page.getByRole("dialog", { name: "Keyboard shortcuts" })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.keyboard.press("j");
  await expect(main.getByRole("region", { name: /^src\/sync\/mod\.rs, lines 61 to 67/ })).toBeVisible();
  await page.keyboard.press("x");
  await expect(banner).toContainText("flows reviewed 1 / 5");
});

test("21:9: the three columns use the full width", async ({ browser }) => {
  const page = await browser.newPage({ viewport: { width: 3440, height: 1440 } });
  await openTour(page);
  const nav = (await page.getByRole("navigation", { name: "Flows and steps" }).boundingBox())!;
  const inspector = (await page.getByRole("complementary", { name: "Inspector" }).boundingBox())!;
  const main = (await page.getByRole("main").boundingBox())!;
  expect(nav.x).toBe(0);
  expect(Math.round(inspector.x + inspector.width)).toBe(3440);
  expect(main.width).toBeGreaterThan(2000);
  await page.close();
});

test("every top-bar control explains itself in a tooltip", async ({ page }) => {
  await openTour(page);
  const banner = page.getByRole("banner").first();
  await expect(banner.getByRole("link", { name: /#482/ })).toHaveAccessibleDescription(/GitHub/);
  await expect(banner.getByText(/^CI:/)).toHaveAccessibleDescription(/Failing checks/);
  await expect(banner.getByText(/^flows reviewed/)).toHaveAccessibleDescription(/No percentage/);
  await expect(banner.getByRole("button", { name: /^Pending review/ })).toHaveAccessibleDescription(
    /pending review/,
  );
  await expect(banner.getByRole("button", { name: /^Help/ })).toHaveAccessibleDescription(/Keyboard/);
  await expect(banner.getByRole("link", { name: /^Settings, signed in/ })).toHaveAccessibleDescription(
    /Settings/,
  );
});

test("widths stored on a 21:9 screen leave the centre usable at 1440 px and after a window resize", async ({
  page,
}) => {
  await page.addInitScript(() =>
    window.localStorage.setItem(
      "smart-review.tour.layout",
      JSON.stringify({ widths: { nav: 1800, inspector: 620 } }),
    ),
  );
  await page.setViewportSize({ width: 3440, height: 1000 });
  await openTour(page);
  expect((await page.getByRole("main").boundingBox())!.width).toBeGreaterThan(900);
  await page.setViewportSize({ width: 1440, height: 900 });
  await expect
    .poll(async () => (await page.getByRole("main").boundingBox())!.width)
    .toBeGreaterThanOrEqual(320);
  await expect(page.locator("main h2:not([data-column-title])").first()).toBeVisible();
});

test("1440: the inspector's classic diff stays inside the column and scrolls on its own", async ({
  page,
}) => {
  await openTour(page, "wrong");
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /^Flow 3:/ })
    .click();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /^Step 1:/ })
    .click();
  const column = page.getByRole("complementary", { name: "Inspector" });
  const diff = column.getByRole("region", { name: /^Classic diff of / });
  await expect(diff).toBeVisible();
  const [box, columnBox] = [await diff.boundingBox(), await column.boundingBox()];
  expect(box!.x + box!.width).toBeLessThanOrEqual(columnBox!.x + columnBox!.width);
  const overflow = await diff.evaluate((el) => el.scrollWidth > el.clientWidth);
  expect(overflow).toBe(true);
});

test("1440: while a comment is written its line reference stays in view", async ({ page }) => {
  await openTour(page, "missed");
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /^Flow 1:/ })
    .click();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /^Step 2:/ })
    .click();
  await page.getByRole("button", { name: "Comment on line 96" }).click();
  const dock = page.getByRole("group", { name: "Ask, feedback and comment" });
  await dock.evaluate((el) => el.scrollTo({ top: el.scrollHeight }));
  await expect(dock.getByText("On src/rpc/client.rs:96")).toBeInViewport({ ratio: 1 });
  await dock.evaluate((el) => el.scrollTo({ top: 0 }));
  await expect(dock.getByText("On src/rpc/client.rs:96")).toBeInViewport({ ratio: 1 });
});

for (const [scenario, outcome] of [
  ["", "Saved to calibration memory"],
  ["&scenario=memcan-failed", "Recorded here; not yet saved to calibration memory (retrying)"],
] as const) {
  test(`feedback shows the MemCan write moving from pending to its outcome${scenario}`, async ({ page }) => {
    await page.goto(`/analyses/${ANALYSIS_ID}?state=wrong${scenario}`);
    await page
      .getByRole("navigation")
      .getByRole("button", { name: /^Flow 3:/ })
      .click();
    await page
      .getByRole("navigation")
      .getByRole("button", { name: /^Step 1:/ })
      .click();
    const memory = page.getByRole("list", { name: "Calibration memory" });
    await expect(memory).toContainText("wrong: Saved to calibration memory");
    await expect(memory).toContainText("overrated: Recorded; saving to calibration memory…");
    await expect(memory).toContainText(`overrated: ${outcome}`, { timeout: 8000 });
  });
}

test("review threads not checked or partial never read as complete", async ({ page }) => {
  await page.goto(`/analyses/${ANALYSIS_ID}?scenario=threads-not-checked`);
  await expect(page.getByRole("region", { name: "Facts" })).toContainText("Review threads: not checked");
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /^Flow 1:/ })
    .click();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /^Step 5:/ })
    .click();
  await expect(page.getByRole("list", { name: "Markers in this step" })).toContainText(
    "Review threads: not checked",
  );
  const inspector = page.getByRole("complementary", { name: "Inspector" });
  await expect(inspector).toContainText("Review threads: not checked");
  await expect(inspector).not.toContainText("No GitHub review threads on these lines.");

  await page.goto(`/analyses/${ANALYSIS_ID}?scenario=threads-partial`);
  // The review resumes at the last viewed step; the facts in question are on PR context.
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /^PR context/ })
    .click();
  await expect(page.getByRole("region", { name: "Facts" })).toContainText(
    "Review threads: the list from GitHub is incomplete",
  );
});
