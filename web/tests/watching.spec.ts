import { expect, test } from "./fixtures";
import { expectAccessible, expectTooltips } from "./screens-support";
import { expectTrustWording } from "./support";

test("watch actions update history and never start an analysis", async ({ page }) => {
  await page.goto("/prs");
  const row = page.getByRole("listitem").filter({ hasText: "acme/node-client #482" });
  const more = row.getByRole("button", { name: "More actions for acme/node-client #482" });
  const mutations: string[] = [];
  page.on("request", (request) => {
    const path = new URL(request.url()).pathname;
    if (request.method() !== "GET" && path.startsWith("/api/")) mutations.push(`${request.method()} ${path}`);
  });
  await more.click();
  await page.getByRole("menuitem", { name: "Stop watching" }).click();
  await expect(row.getByText(/^watching since/)).toHaveCount(0);
  await more.click();
  await page.getByRole("menuitem", { name: "Watch", exact: true }).click();
  await expect(row.getByText(/^watching since/)).toBeVisible();
  expect(mutations).toEqual([
    "DELETE /api/v1/prs/acme/node-client/482/watch",
    "PUT /api/v1/prs/acme/node-client/482/watch",
  ]);
  await expectTooltips(page);
  await expectTrustWording(page);
  await expectAccessible(page);
  await page.emulateMedia({ colorScheme: "dark" });
  await expectAccessible(page);
});

test("invalid input is announced and the Open tooltip is available on keyboard focus", async ({ page }) => {
  await page.goto("/prs");
  const input = page.getByLabel("GitHub pull request URL");
  await input.fill("https://github.com/acme/node-client/issues/482");
  await input.press("Enter");
  await expect(page.getByRole("alert")).toContainText("Enter a GitHub pull request link");
  await expect(input).toHaveAccessibleDescription(/Enter a GitHub pull request link/);
  await input.press("Tab");
  await expect(page.getByRole("button", { name: "Open", exact: true })).toBeFocused();
  await expect(page.getByRole("tooltip", { name: /Opens the pull request page/ })).toBeVisible();
});
