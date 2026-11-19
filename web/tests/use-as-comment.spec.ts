import { expect, test } from "./fixtures";
import { expectAccessible, expectKeyboardTooltips } from "./screens-support";
import { ANALYSIS_ID } from "./support";

// Owner request F31: a flag or an Ask answer opens the comment composer prefilled; nothing is posted by it.
test("Use as review comment prefills the pending-review composer from a flag and an Ask answer", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  const posts: string[] = [];
  page.on("request", (r) => {
    if (r.method() !== "GET" && r.url().includes("/pending-review")) posts.push(r.url());
  });
  await page.goto(`/analyses/${ANALYSIS_ID}?scenario=ask-thread#f1.5`);
  const inspector = page.getByRole("complementary", { name: "Inspector" });
  const card = inspector.locator('[data-annotation-id="f1.5.a3"]');
  const fromFlag = card.getByRole("button", { name: "Use as review comment" });
  await fromFlag.focus();
  await page.keyboard.press("Enter");
  const composer = inspector.getByRole("textbox", { name: /^Comment on src\/rpc\/client\.rs:/ });
  await expect(composer).toBeFocused();
  const text = await composer.inputValue();
  expect(text.split("\n").slice(0, 3)).toEqual([
    "LLM-suggested — review before posting",
    "",
    "Potential double broadcast on a late reply:",
  ]);
  expect(text).toMatch(/\n1\. .+\n2\. /);
  expect(text).toContain("Not checked: Not reproduced against a node; no test was run.");
  await composer.press("ControlOrMeta+End");
  await composer.pressSequentially(" My note.");

  // An Ask answer on the same step goes after the open draft, after a blank line.
  const thread = inspector.getByRole("list", { name: "Ask conversation" });
  await thread.getByRole("button", { name: "Use as review comment" }).first().click();
  const appended = `${text} My note.\n\nLLM-suggested — review before posting\n\n`;
  await expect.poll(() => composer.inputValue()).toContain(appended);
  const both = await composer.inputValue();
  expect(both.startsWith(appended)).toBe(true);
  expect(both).toContain("1. The first reply is late.");
  expect(posts).toEqual([]);
  await expectAccessible(page);
  await expectKeyboardTooltips(page);
});
