import { expect, test } from "./fixtures";
import { expectAccessible, expectTooltips } from "./screens-support";
import { expectTrustWording } from "./support";

test("paste a PR link with the keyboard, add through Watch only, and explain duplicates", async ({
  page,
}) => {
  await page.goto("/watching");
  const input = page.getByRole("textbox", { name: "GitHub pull request link" });
  await expect(input).toBeVisible();
  await page.getByRole("button", { name: "Unwatch acme/node-client #482" }).click();
  await expect(page.getByRole("button", { name: "Unwatch acme/node-client #482" })).toHaveCount(0);
  const mutations: string[] = [];
  page.on("request", (request) => {
    if (request.method() !== "GET" && new URL(request.url()).pathname.startsWith("/api/"))
      mutations.push(`${request.method()} ${new URL(request.url()).pathname}`);
  });
  await input.fill("https://github.com/acme/node-client/pull/482/files?diff=split#L12");
  await input.press("Enter");
  await expect(page.getByRole("status")).toHaveText("Watching acme/node-client #482.");
  await expect(page.getByRole("button", { name: "Unwatch acme/node-client #482" })).toBeVisible();
  await expect(input).toHaveValue("");
  await expect(input).toBeFocused();
  expect(mutations).toEqual(["PUT /api/v1/prs/acme/node-client/482/watch"]);
  await input.fill("https://github.com/ACME/Node-Client/pull/482");
  await input.press("Enter");
  await expect(page.getByRole("alert")).toContainText("already watched");
  await expect(input).toHaveAttribute("aria-invalid", "true");
  expect(mutations).toHaveLength(1);
  await expectTooltips(page);
  await expectTrustWording(page);
  await expectAccessible(page);
  await page.emulateMedia({ colorScheme: "dark" });
  await expectAccessible(page);
});

test("invalid input is announced and the Watch tooltip works on keyboard focus", async ({ page }) => {
  await page.goto("/watching");
  const input = page.getByRole("textbox", { name: "GitHub pull request link" });
  await input.fill("https://github.com/acme/node-client/issues/482");
  await input.press("Enter");
  await expect(page.getByRole("alert")).toContainText("Enter a GitHub pull request link");
  await expect(input).toHaveAccessibleDescription(/Enter a GitHub pull request link/);
  await input.press("Tab");
  await expect(page.getByRole("button", { name: "Watch pull request" })).toBeFocused();
  await expect(page.getByRole("tooltip", { name: /Adds this public pull request/ })).toBeVisible();
});
