import { expect, test } from "./fixtures";
import { expectAccessible } from "./screens-support";

test("GitHub comment markup is readable without links, images or agent prompts", async ({ page }) => {
  await page.goto("/pr/acme/node-client/482/comments?item=C14");
  const quote = page.locator("[data-comment-text]");
  await expect(quote).toBeVisible();
  // Change only the in-memory msw source; Read GitHub again exercises the regular comment display path.
  await page.evaluate(async () => {
    const module = "/src/mocks/review-loop.ts";
    const { listItems } = await import(module);
    listItems().find((item: { no: number }) => item.no === 14).body = [
      "<!-- coderabbit-ai-generated:v1 -->",
      '**<picture><img src="https://evil.example/pixel"></picture> Minor** · **Functional Correctness**',
      "Keep `nonce` unique. See [docs](https://github.com/acme/node-client).",
      "```html",
      "</details>",
      "```",
      "<!-- ai-agent-prompt:start -->",
      "<details><summary>Prompt to fix issue</summary>Do not show this.</details>",
      "<!-- ai-agent-prompt:end -->",
    ].join("\n");
  });
  await page.getByRole("button", { name: "Read GitHub again" }).click();
  await expect(quote).toContainText("Minor · Functional Correctness");
  await expect(quote).toContainText("docs (https://github.com/acme/node-client)");
  await expect(quote.locator("code").first()).toHaveText("nonce");
  await expect(quote.locator("pre code")).toHaveText("</details>\n");
  await expect(quote).not.toContainText(/coderabbit-ai|Prompt to fix|Do not show|<picture>/);
  await expect(quote.locator("a,img,picture,details")).toHaveCount(0);
  await expectAccessible(page);
});
