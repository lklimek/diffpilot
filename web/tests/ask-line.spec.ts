import { expect, test } from "./fixtures";
import { expectAccessible } from "./screens-support";
import { ANALYSIS_ID } from "./support";

// Owner request F32: "Ask the LLM about this line" puts path:line into the Ask field, opens and focuses Ask, sends nothing.
test("Ask the LLM about this line fills the Ask field from the code view and the classic diff", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`/analyses/${ANALYSIS_ID}?scenario=ask-thread#f1.2`);
  const main = page.getByRole("main");
  const inspector = page.getByRole("complementary", { name: "Inspector" });
  const field = inspector.getByRole("textbox", { name: /^Ask the LLM about / });
  const posts: string[] = [];
  page.on("request", (r) => {
    if (r.method() === "POST" && r.url().includes("/questions")) posts.push(r.url());
  });

  // Ask collapsed first: the action opens it.
  await inspector.getByRole("button", { name: /^▾ Ask about/ }).click();
  await expect(field).toBeHidden();
  // Keyboard: Tab from a line's comment action reaches its Ask action, which shows on focus.
  await main.getByRole("button", { name: "Comment on line 89" }).focus();
  await page.keyboard.press("Tab");
  const action = main.getByRole("button", { name: "Ask the LLM about line 89" });
  await expect(action).toBeFocused();
  await expect(action).toHaveCSS("opacity", "1");
  await page.keyboard.press("Enter");
  await expect(field).toBeFocused();
  await expect(field).toHaveValue("src/rpc/client.rs:89 ");
  await page.keyboard.type("is this the retry?");
  await expect(field).toHaveValue("src/rpc/client.rs:89 is this the retry?");

  // The classic diff in the Inspector offers it too; it goes in at the cursor.
  await field.evaluate((e: HTMLTextAreaElement) => e.setSelectionRange(0, 0));
  const diff = inspector.getByRole("region", { name: /^Classic diff of / });
  await diff
    .getByRole("button", { name: /^Ask the LLM about line \d+$/ })
    .first()
    .click();
  await expect(field).toHaveValue(/^src\/rpc\/client\.rs:\d+ src\/rpc\/client\.rs:89 is this the retry\?$/);
  await expect(field).toBeFocused();
  expect(posts).toEqual([]);
  await expectAccessible(page);
});
