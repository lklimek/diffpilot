// Journey (h): browser security smoke — the Content-Security-Policy header is sent and blocks inline script; LLM
// analysis text with markup renders as text. GitHub text and Ask answers: comments.spec.ts, ask.spec.ts.
import { readFileSync } from "node:fs";
import { checkScreen, finishedReview, heading, inspector, openStep } from "../support/app";
import { expect, test } from "../support/fixtures";

const MARKUP = '<img src=x onerror="window.__xss=1"><script>window.__xss=2</script>';

test("CSP header present; an inline script does not execute", async ({ page }) => {
  const response = await page.goto("/");
  const csp = response?.headers()["content-security-policy"] ?? "";
  // Scripts fall under script-src, or default-src without it.
  const scripts =
    /(?:^|;)\s*script-src ([^;]*)/.exec(csp)?.[1] ?? /(?:^|;)\s*default-src ([^;]*)/.exec(csp)?.[1];
  expect(scripts, `script sources in "${csp}"`).toBe("'self'");
  const violations = page.evaluate(
    () =>
      new Promise<string>((resolve) =>
        document.addEventListener("securitypolicyviolation", (event) => resolve(event.violatedDirective), {
          once: true,
        }),
      ),
  );
  await page.evaluate(() => {
    const script = document.createElement("script");
    script.textContent = "window.__inline = 1";
    document.body.append(script);
  });
  expect(await violations).toMatch(/^script-src/);
  expect(await page.evaluate(() => (window as { __inline?: number }).__inline)).toBeUndefined();
  await checkScreen(page);
});

test("security: LLM analysis text with markup renders as text", async ({ page, stack }) => {
  test.skip(!stack.script, "needs the fake provider's script (a local stack)");
  const flow = JSON.parse(
    readFileSync(new URL("../../fixtures/pr-482/agent/flow-1.json", import.meta.url), "utf8"),
  ) as { steps: { title: string; why: string }[] };
  flow.steps[0].title = `${MARKUP} trigger`;
  flow.steps[0].why = `${MARKUP} why`;
  stack.script?.([{ id: "markup", job: "analyse_flow", flow: 1, action: { answer: { output: flow } } }]);
  const dialogs: string[] = [];
  page.on("dialog", (dialog) => {
    dialogs.push(dialog.message());
    void dialog.dismiss();
  });
  await finishedReview(page);
  await openStep(page, 1, 1);
  await expect(heading(page)).toContainText(`${MARKUP} trigger`);
  await expect(page.getByRole("main")).toContainText(`${MARKUP} why`);
  await expect(inspector(page)).toContainText(`${MARKUP} trigger`);
  await expect(page.locator("#root img, #root script, #root iframe, #root object")).toHaveCount(0);
  const handlers = await page
    .locator("#root")
    .evaluate((root) =>
      [...root.querySelectorAll("*")].flatMap((node) =>
        [...node.attributes].filter((a) => a.name.startsWith("on")).map((a) => `${node.tagName} ${a.name}`),
      ),
    );
  expect(handlers).toEqual([]);
  expect(await page.evaluate(() => (window as { __xss?: number }).__xss)).toBeUndefined();
  expect(dialogs).toEqual([]);
  await checkScreen(page);
});
