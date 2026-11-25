import { expect, test } from "./fixtures";
import { expectAccessible } from "./screens-support";
import { ANALYSIS_ID } from "./support";

// Owner request F32: "Ask about this line" opens the Ask chat about that line and sends nothing.
test("Ask about this line opens the chat about the line from the code view and the classic diff", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`/analyses/${ANALYSIS_ID}?scenario=ask-thread#f1.2`);
  const main = page.getByRole("main");
  const inspector = page.getByRole("complementary", { name: "Inspector" });
  const chat = page.getByRole("dialog", { name: "Ask" });
  const field = chat.getByRole("textbox", { name: "Your question" });
  const posts: string[] = [];
  page.on("request", (r) => {
    if (r.method() === "POST" && r.url().includes("/questions")) posts.push(r.url());
  });

  // Keyboard: Tab from a line's comment action reaches its Ask action, which shows on focus.
  await main.getByRole("button", { name: "Comment on line 89" }).focus();
  await page.keyboard.press("Tab");
  const action = main.getByRole("button", { name: "Ask the LLM about line 89" });
  await expect(action).toBeFocused();
  await expect(action).toHaveCSS("opacity", "1");
  await page.keyboard.press("Enter");
  await main.getByRole("button", { name: "Ask about this line" }).click();
  await expect(field).toBeFocused();
  await expect(chat).toContainText(/About:\s*Step 1\.2 · client\.rs 89/);
  await page.keyboard.type("is this the retry?");

  // The classic diff in the Inspector offers it too; the draft stays.
  const diff = inspector.getByRole("region", { name: /^Classic diff of / });
  await diff
    .getByRole("button", { name: /^Ask the LLM about line \d+$/ })
    .first()
    .click();
  await diff.getByRole("button", { name: "Ask about this line" }).click();
  await expect(chat).toContainText(/About:\s*Step 1\.2 · client\.rs 88/);
  await expect(field).toHaveValue("is this the retry?");
  await expect(field).toBeFocused();
  expect(posts).toEqual([]);
  await expectAccessible(page);
});
