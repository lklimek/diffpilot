import { expect, test } from "./fixtures";
import { ANALYSIS_ID, expectTrustWording, openTour } from "./support";

const heading = (page: import("@playwright/test").Page) =>
  page.locator("main h2:not([data-column-title])").first();

test("PR URL → PR context → flow 1 introduction → step 1 via j", async ({ page }) => {
  await page.goto("/review");
  await page.getByLabel("GitHub pull request URL").fill("https://github.com/acme/node-client/pull/482");
  await page.getByRole("button", { name: "Check" }).click();
  await page.getByRole("button", { name: "Start analysis", exact: true }).click();
  await expect(page).toHaveURL(/\/pr\/acme\/node-client\/482$/);
  await page.getByRole("link", { name: "Open", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/analyses/${ANALYSIS_ID}#pr_context$`));
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
  await page.goto("/review");
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
  await expect(page.getByRole("tooltip").filter({ visible: true })).toHaveCount(0);
  await page.locator('[data-mark="remark"]').first().hover();
  await expect(page.getByRole("dialog", { name: /^LLM remarks? at line/ })).toContainText("Was: ");
  const summary = page.getByRole("button", { name: /Assessment$/ });
  await expect(summary).toHaveAttribute("aria-expanded", "false");
  await summary.click();
  await expect(summary).toHaveAttribute("aria-expanded", "true");
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

test("a step shows its LLM flags as cards without opening the collapsed assessment", async ({ page }) => {
  await openTour(page, "default");
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Flow 1: Send an RPC request with retry" })
    .click();
  await page.getByRole("navigation").getByRole("button", { name: "Step 5: Retry allowed?" }).click();
  const inspector = page.getByRole("complementary", { name: "Inspector" });
  await expect(inspector.getByRole("button", { name: "Flags (1)" })).toHaveAttribute("aria-expanded", "true");
  await expect(inspector.getByRole("button", { name: /Assessment$/ })).toHaveAttribute(
    "aria-expanded",
    "false",
  );
  const card = inspector.locator('[data-annotation-id="f1.5.a3"]');
  await expect(card.getByText("⚑ Red flag")).toBeVisible();
  await expect(card.getByText("Potential double broadcast on a late reply:")).toBeVisible();
  await expect(card.getByRole("list", { name: "Scenario" }).getByRole("listitem")).toHaveCount(4);
  await expect(card).toContainText("Not checked: Not reproduced against a node; no test was run.");
});

test("noq: the step hides Flags and Assessment when the LLM gave none", async ({ page }) => {
  await openTour(page, "noq");
  await page
    .getByRole("navigation")
    .getByRole("button", { name: "Flow 1: Send an RPC request with retry" })
    .click();
  await page.getByRole("navigation").getByRole("button", { name: "Step 5: Retry allowed?" }).click();
  const inspector = page.getByRole("complementary", { name: "Inspector" });
  await expect(inspector.getByRole("button", { name: /^Flags/ })).toHaveCount(0);
  await expect(inspector.getByRole("button", { name: /Assessment$/ })).toHaveCount(0);
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
  // The bottom edge drags the diagram taller.
  const body = diagram.locator("[data-diagram-body]");
  const before = (await body.boundingBox())!.height;
  const edge = (await diagram.getByRole("separator", { name: "Resize the flow diagram" }).boundingBox())!;
  expect(edge.height).toBeGreaterThanOrEqual(24);
  await page.mouse.move(edge.x + edge.width / 2, edge.y + edge.height / 2);
  await page.mouse.down();
  await page.mouse.move(edge.x + edge.width / 2, edge.y + edge.height / 2 + 120, { steps: 6 });
  await page.mouse.up();
  await expect.poll(async () => (await body.boundingBox())!.height).toBeGreaterThan(before + 80);
  await expectInert();

  // Step 1: title (heading and menu aria-label), why, inspector, the pinned LLM answer.
  await page.keyboard.press("j");
  await expect(heading(page)).toContainText(`${markup} trigger`);
  await expect(nav.getByRole("button", { name: `Step 1: ${markup} trigger` })).toBeVisible();
  await expect(inspector.getByText(`${markup} answer`)).toBeVisible();
  await expectInert();

  // Step 5: the highlight has no tooltip (was → is is in the remark popover) and the thread marker's popover.
  await nav.getByRole("button", { name: /^Step 5:/ }).click();
  const mark = page.locator("[role=mark]").first();
  await mark.hover();
  await expect(page.getByRole("tooltip").filter({ visible: true })).toHaveCount(0);
  await page.locator('[data-mark="remark"]').first().hover();
  await expect(page.getByRole("dialog", { name: /^LLM remarks? at line/ })).toContainText(`Was: ${markup}`);
  await page.locator('[data-mark="thread"]').first().hover();
  const thread = page.getByRole("dialog", { name: /^GitHub threads? at line/ });
  await expect(thread).toBeVisible();
  await expect(thread).toContainText("@bob");
  await expect(thread).not.toContainText(/<img|<script|window\.__xss/);
  await expectInert();
  expect(dialogs).toEqual([]);
});

test("columns: hide, full screen and widths are remembered; a click anywhere on a strip restores", async ({
  page,
}) => {
  await openTour(page);
  const separator = page.getByRole("separator", { name: /Resize the Flows column/ });
  // The Tour removes the page scrollbar gutter; wait for its ResizeObserver measurement.
  await page.evaluate(
    () => new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))),
  );
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
  await expect(page.getByRole("group", { name: "Feedback and comment" })).toBeVisible();
  await page.getByRole("button", { name: "Full screen Review column" }).click();
  await expect(page.getByRole("button", { name: "Show Flows column" })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("button", { name: "Hide Flows column" })).toBeVisible();
});

test("reload keeps the position and the ticks", async ({ page }) => {
  await openTour(page);
  await page.keyboard.press("j");
  await page.keyboard.press("j");
  const saved = page.waitForResponse(
    (response) =>
      response.url().endsWith("/review-state") &&
      response.request().method() === "PUT" &&
      response.status() === 204 &&
      response.request().postDataJSON().ticked_steps.includes("f1.1"),
  );
  await page.keyboard.press("x");
  await expect(heading(page)).toHaveText("1 Trigger: callers of send");
  await saved;
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
  const ask = page.getByRole("dialog", { name: "Ask" }).getByRole("textbox", { name: "Your question" });
  await expect(ask).toBeFocused();
  // Esc hides the chat and gives the keys back to the page.
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog", { name: "Ask" })).toHaveCount(0);
  await page.keyboard.press("?");
  await expect(page.getByRole("dialog", { name: "Keyboard shortcuts" })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.keyboard.press("j");
  await expect(main.getByRole("region", { name: /^src\/sync\/mod\.rs, lines 61 to 67/ })).toBeVisible();
  await page.keyboard.press("x");
  await expect(banner).toContainText("Your progress 1 / 5");
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
  await expect(
    banner.getByRole("link", { name: /^Pull request #482 on GitHub/ }),
  ).toHaveAccessibleDescription(/GitHub/);
  await expect(banner.getByRole("link", { name: "acme/node-client #482" })).toHaveAccessibleDescription(
    /PR context/,
  );
  await expect(banner.getByRole("button", { name: "View: Flows" })).toHaveAccessibleDescription(/Files/);
  // The chip (link or span) carries the tooltip; its text may sit in an inner truncating span.
  await expect(
    banner
      .locator("[aria-describedby]")
      .filter({ hasText: /^Check runs:/ })
      .first(),
  ).toHaveAccessibleDescription(/Failing checks/);
  await expect(banner.getByText(/^Your progress/)).toHaveAccessibleDescription(/No percentage/);
  await expect(banner.getByRole("button", { name: /^Pending review/ })).toHaveAccessibleDescription(
    /pending review/,
  );
  const user = banner.getByRole("button", { name: "alice: account and LLM activity", exact: true });
  await expect(user).toHaveAccessibleDescription("Signed in as alice (GitHub)");
  await user.click();
  await expect(banner.getByRole("link", { name: "Settings" })).toHaveAttribute("href", "/settings");
  await banner.getByRole("button", { name: "Keyboard shortcuts" }).click();
  await expect(page.getByRole("dialog", { name: "Keyboard shortcuts" })).toBeVisible();
});

test("widths stored on a 21:9 screen leave the centre usable at 1440 px and after a window resize", async ({
  page,
}) => {
  await page.addInitScript(() =>
    window.localStorage.setItem(
      "diffpilot.tour.layout",
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
  const dock = page.getByRole("group", { name: "Feedback and comment" });
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
    await expect(memory).toContainText("wrong: Recorded");
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

test("right after a click on a nav row the next row is clickable, without moving the mouse away", async ({
  page,
}) => {
  await openTour(page);
  const nav = page.getByRole("navigation");
  for (const [row, next] of [
    [/^PR context/, /^Flow 1:/],
    [/^Unexplained/, /^Moves with identical text/],
    [/^Moves with identical text/, /^Tests/],
  ] as const) {
    const clicked = nav.getByRole("button", { name: row });
    await clicked.click();
    // The clicked row keeps the focus under the pointer, without a tip over the rows below.
    await expect(clicked).toBeFocused();
    await expect(page.getByRole("tooltip").filter({ visible: true })).toHaveCount(0);
    // No hover away, no force: a tip covering the next row would intercept this click.
    await nav.getByRole("button", { name: next }).click({ timeout: 3000 });
    await expect(nav.getByRole("button", { name: next })).toHaveAttribute("aria-current", /.+/);
  }
});

test("a binary file is listed in Not analysed with its badge and tooltip", async ({ page }) => {
  await page.goto(`/analyses/${ANALYSIS_ID}?scenario=binary-file`);
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /^Mechanical & generated/ })
    .click();
  const block = page.getByRole("main").getByRole("article");
  await expect(block).toContainText("assets/logo.png whole file · rule binary file (git)");
  await block.getByText("binary file", { exact: true }).hover();
  await expect(page.getByRole("tooltip").filter({ visible: true })).toHaveText(
    "Git treats this file as binary; it is not shown or analysed.",
  );
});

test("at the first block, Previous is reachable by keyboard and its tip says why it does nothing", async ({
  page,
}) => {
  await openTour(page);
  const previous = page.getByRole("main").getByRole("button", { name: "← Previous" });
  await expect(previous).toHaveAttribute("aria-disabled", "true");
  await page
    .getByRole("main")
    .getByRole("heading", { name: /^PR context:/ })
    .click();
  await previous.focus();
  await expect(previous).toBeFocused();
  await expect(page.getByRole("tooltip").filter({ visible: true })).toHaveText(
    "This is the first block; there is none before it.",
  );
  await page.keyboard.press("Enter");
  await expect(page.locator("main h2:not([data-column-title])").first()).toHaveText(
    "PR context: acme/node-client #482",
  );
});

test("Next and j tick the current step; Previous and the flow introduction do not", async ({ page }) => {
  await openTour(page);
  const nav = page.getByRole("navigation");
  const putWith = (id: string) =>
    page.waitForResponse(
      (r) =>
        r.url().endsWith("/review-state") &&
        r.request().method() === "PUT" &&
        r.status() === 204 &&
        r.request().postDataJSON().ticked_steps.includes(id),
    );
  await nav.getByRole("button", { name: "Flow 1: Send an RPC request with retry" }).click();
  await page.keyboard.press("j"); // from the introduction: no tick
  await expect(heading(page)).toHaveText("1 Trigger: callers of send");
  await expect(nav.getByRole("button", { name: "Step 1: Trigger: callers of send" })).toBeVisible();
  const first = putWith("f1.1");
  await page.getByRole("button", { name: /^Next: / }).click();
  await first;
  await expect(nav.getByRole("button", { name: /^Step 1: .*, reviewed$/ })).toBeVisible();
  const second = putWith("f1.2");
  await page.keyboard.press("j");
  await second;
  await expect(nav.getByRole("button", { name: /^Step 2: .*, reviewed$/ })).toBeVisible();
  await page.keyboard.press("k");
  await page.keyboard.press("k");
  await expect(nav.getByRole("button", { name: /^Step 1: .*, reviewed$/ })).toBeVisible();
  await expect(nav.getByRole("button", { name: /^Step 3: .*, reviewed$/ })).toHaveCount(0);
});
