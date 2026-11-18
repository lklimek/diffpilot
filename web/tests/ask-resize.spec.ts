import { expect, test } from "@playwright/test";
import { expectAccessible } from "./screens-support";
import { ANALYSIS_ID } from "./support";

// Owner request F28: the Ask area grows from its top edge; the sections above shrink; kept with the layout.
test("the Ask area resizes by dragging or the keyboard, is remembered, and resets", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`/analyses/${ANALYSIS_ID}?scenario=ask-thread#f1.5`);
  const inspector = page.getByRole("complementary", { name: "Inspector" });
  const dock = inspector.getByRole("group", { name: "Ask, feedback and comment" });
  const thread = inspector.getByRole("region", { name: "Scrollable conversation" });
  const handle = inspector.getByRole("separator", { name: "Resize the Ask area" });
  const before = (await dock.boundingBox())!;
  const threadBefore = (await thread.boundingBox())!.height;
  const edge = (await handle.boundingBox())!;
  expect(edge.height).toBeGreaterThanOrEqual(24);
  await page.mouse.move(edge.x + edge.width / 2, edge.y + edge.height / 2);
  await page.mouse.down();
  await page.mouse.move(edge.x + edge.width / 2, edge.y + edge.height / 2 - 200, { steps: 6 });
  await page.mouse.up();
  await expect.poll(async () => (await dock.boundingBox())!.height).toBeGreaterThan(before.height + 150);
  // The conversation takes the room; the dock stays at the Inspector's bottom.
  expect((await thread.boundingBox())!.height).toBeGreaterThan(threadBefore + 100);
  const after = (await dock.boundingBox())!;
  expect(Math.abs(after.y + after.height - (before.y + before.height))).toBeLessThanOrEqual(1);
  await expectAccessible(page);

  // Remembered over a reload.
  await page.reload();
  await expect
    .poll(async () => Math.round((await dock.boundingBox())!.height))
    .toBe(Math.round(after.height));

  // Keyboard: Up grows, Enter resets.
  await handle.focus();
  await page.keyboard.press("ArrowUp");
  await expect.poll(async () => (await dock.boundingBox())!.height).toBeGreaterThan(after.height + 30);
  await page.keyboard.press("Enter");
  await expect
    .poll(async () => Math.round((await dock.boundingBox())!.height))
    .toBe(Math.round(before.height));

  // A collapsed Ask has no handle and no empty room.
  await page.keyboard.press("ArrowUp");
  await inspector.getByRole("button", { name: /^▾ Ask about/ }).click();
  await expect(handle).toHaveCount(0);
  expect((await dock.boundingBox())!.height).toBeLessThan(before.height);
  await inspector.getByRole("button", { name: /^▸ Ask about/ }).click();
  await expect(handle).toBeVisible();
});

// A small dock with the comment composer leading it: the Ask heading and field never paint over their neighbours;
// only the conversation gives up room, and the dock scrolls.
test("a small Ask dock with a comment being written scrolls instead of overlapping", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.addInitScript(() => {
    const key = "smart-review.tour.layout";
    const stored = JSON.parse(window.localStorage.getItem(key) ?? "null") as Record<string, unknown> | null;
    window.localStorage.setItem(
      key,
      JSON.stringify({
        hidden: { nav: false, main: false, inspector: false },
        full: null,
        widths: { nav: null, inspector: null },
        diagram: false,
        diagramHeight: null,
        ...stored,
        askHeight: 160,
      }),
    );
  });
  await page.goto(`/analyses/${ANALYSIS_ID}?scenario=ask-thread#f1.2`);
  const inspector = page.getByRole("complementary", { name: "Inspector" });
  await page.getByRole("main").getByRole("button", { name: "Comment on line 92" }).click();
  const composer = inspector.getByRole("textbox", { name: /^Comment on / });
  await expect(composer).toBeVisible();
  const parts = [
    composer,
    inspector.getByRole("button", { name: /^▾ Ask about/ }),
    inspector.getByRole("textbox", { name: /^Ask the LLM about / }),
    inspector.getByRole("group", { name: /^Feedback on / }),
  ];
  const dock = inspector.getByRole("group", { name: "Ask, feedback and comment" });
  for (const part of parts) {
    await part.scrollIntoViewIfNeeded();
    await expect(part).toBeVisible();
  }
  // In the dock's scroll coordinates, each part ends before the next one starts.
  const tops = await Promise.all(
    parts.map((part) =>
      part.evaluate((e) => {
        const box = e.getBoundingClientRect();
        const scroller = e.closest("[data-dock]")!;
        const offset = scroller.getBoundingClientRect().top - scroller.scrollTop;
        return { top: box.top - offset, bottom: box.bottom - offset };
      }),
    ),
  );
  for (let i = 1; i < tops.length; i++) expect(tops[i]!.top).toBeGreaterThanOrEqual(tops[i - 1]!.bottom - 1);
  expect(await dock.evaluate((e) => e.scrollHeight > e.clientHeight)).toBe(true);
});
