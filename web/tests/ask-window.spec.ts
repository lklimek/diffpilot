import { expect, test } from "./fixtures";
import { ANALYSIS_ID } from "./support";

const flow = `/analyses/${ANALYSIS_ID}?scenario=ask-thread#f1.5`;
const launcher = /^Ask.*: open your private chat/;

test("position can be dragged, restored, moved by keyboard or chosen from the menu", async ({ page }) => {
  await page.goto(flow);
  await page.getByRole("button", { name: launcher }).click();
  const chat = page.getByRole("dialog", { name: "Ask" });
  const grip = chat.getByRole("button", { name: /^Move the Ask window/ });
  const start = (await chat.boundingBox())!;
  await grip.press("ArrowLeft");
  await grip.press("Shift+ArrowUp");
  const moved = (await chat.boundingBox())!;
  expect(moved.x).toBe(start.x - 10);
  expect(moved.y).toBe(start.y - 50);

  const handle = (await grip.boundingBox())!;
  await page.mouse.move(handle.x + 10, handle.y + 10);
  await page.mouse.down();
  await page.mouse.move(1, 1);
  await page.mouse.up();
  const dragPosition = (await chat.boundingBox())!;
  expect(dragPosition.x).toBeLessThan(moved.x);
  expect(dragPosition.y).toBeGreaterThanOrEqual(0);
  await chat.getByRole("separator", { name: /^Chat height/ }).press("ArrowUp");
  const dragged = (await chat.boundingBox())!;
  expect(dragged.height).toBe(dragPosition.height + 20);

  await page.reload();
  await expect(chat).toBeVisible();
  expect(await chat.boundingBox()).toEqual(dragged);
  await page.setViewportSize({ width: 700, height: 480 });
  const clamped = (await chat.boundingBox())!;
  expect(clamped.x).toBeGreaterThanOrEqual(0);
  expect(clamped.y).toBeGreaterThanOrEqual(0);
  expect(clamped.x + clamped.width).toBeLessThanOrEqual(700);
  expect(clamped.y + clamped.height).toBeLessThanOrEqual(480);
  await page.setViewportSize({ width: 1440, height: 900 });
  const position = chat.getByRole("button", { name: "Position" });
  await position.press("ArrowDown");
  await page.keyboard.press("Escape");
  await expect(position).toBeFocused();
  await position.click();
  await chat.getByRole("menuitemradio", { name: "Top left" }).click();
  expect((await chat.boundingBox())!.x).toBeLessThan(20);
  expect((await chat.boundingBox())!.y).toBeLessThan(20);
  await chat.getByRole("button", { name: "Full height" }).click();
  expect((await chat.boundingBox())!.x).toBeLessThan(20);
  await grip.press("Home");
  expect((await chat.boundingBox())!.x).toBe(start.x);
  await grip.dblclick();
  expect(await chat.boundingBox()).toEqual(start);
});

test("the phone sheet opens at half height and expands without desktop controls", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(flow);
  await page.getByRole("button", { name: launcher }).click();
  const chat = page.getByRole("dialog", { name: "Ask" });
  const half = (await chat.boundingBox())!;
  expect(half).toMatchObject({ x: 0, width: 390, height: 422 });
  await expect(chat.getByRole("button", { name: "Position" })).toHaveCount(0);
  await expect(chat.getByRole("button", { name: /^Move the Ask window/ })).toHaveCount(0);
  await chat.getByRole("button", { name: "Full", exact: true }).click();
  expect((await chat.boundingBox())!.height).toBeGreaterThan(half.height);
  await chat.getByRole("button", { name: "Half", exact: true }).click();
  expect(await chat.boundingBox()).toEqual(half);
});

for (const view of ["Flows", "Files"] as const) {
  test(`opening Ask from a remark in ${view} prefills a private draft and requires a question`, async ({
    page,
  }) => {
    const files = `/analyses/${ANALYSIS_ID}/files?scenario=ask-thread#lines?file=src/rpc/client.rs&side=head&line=99`;
    await page.goto(view === "Flows" ? flow : files);
    const posts: unknown[] = [];
    page.on("request", (r) => {
      if (r.method() === "POST" && r.url().includes("/questions")) posts.push(r.postDataJSON());
    });
    const marker = page.getByRole("main").getByRole("button", { name: /^LLM remark at line 99: Red flag/ });
    await marker.click();
    const remark = page.getByRole("dialog", { name: /^LLM remark at line 99: Red flag/ });
    await remark.getByRole("button", { name: "Ask about this remark" }).click();
    const chat = page.getByRole("dialog", { name: "Ask" });
    const field = chat.getByRole("textbox", { name: "Your question" });
    await expect(field).toBeFocused();
    await expect(field).toHaveValue(/^> LLM remark \(Red flag, line 99\): .+\n\n$/);
    const draft = await field.inputValue();
    expect(await field.evaluate((el: HTMLTextAreaElement) => el.selectionStart)).toBe(draft.length);
    await field.press("Enter");
    await expect(chat.getByRole("alert")).toHaveText("Write a question first.");
    expect(posts).toEqual([]);
    await field.pressSequentially("Why does it matter?");
    await field.press("Enter");
    await expect
      .poll(() => posts)
      .toEqual([
        {
          text: `${draft}Why does it matter?`,
          anchor:
            view === "Flows"
              ? { kind: "step", step_id: "f1.5", lines: { start_line: 99, end_line: 99 } }
              : {
                  kind: "lines",
                  path: "src/rpc/client.rs",
                  side: "head",
                  range: { start_line: 99, end_line: 99 },
                },
        },
      ]);
    const anchor = chat.locator("[data-thread]").last().getByRole("button", { name: "Remark · line 99" });
    await expect(anchor).toBeVisible();
    await chat.getByRole("button", { name: "Hide the Ask chat" }).click();
    await expect(marker).toBeFocused();
    await marker.click();
    const id = await marker.getAttribute("data-annotation");
    await remark.getByRole("button", { name: /^Show in Inspector/ }).click();
    const inspector = page.getByRole("complementary", { name: "Inspector" });
    await inspector
      .locator(`[data-annotation-id="${id}"]`)
      .getByRole("button", { name: "Ask about this remark" })
      .click();
    await expect(field).toHaveValue(draft);
    await expect(field).toBeFocused();
    expect(posts).toHaveLength(1);
  });
}
