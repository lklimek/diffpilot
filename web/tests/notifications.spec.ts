import type { Page } from "@playwright/test";
import { expect, test } from "./fixtures";
import { ANALYSIS_ID } from "./support";

// Browser notifications when work finishes in a tab the user is not looking at (owner request, 2026-11-19).
// The fixture repository has the short name "NC" in the dashboard settings. Headless Chromium reports notifications
// as denied whatever is granted, so the API is replaced by a recorder whose permission the "browser" grants once
// asked (kept in sessionStorage across reloads).

declare global {
  interface Window {
    __notifications: { title: string; options: NotificationOptions; onclick: ((e: Event) => void) | null }[];
  }
}

/** Records notifications instead of showing them; `focused` false puts the window in the background. */
async function recordNotifications(page: Page, { optedIn, focused }: { optedIn: boolean; focused: boolean }) {
  await page.addInitScript(
    ({ optedIn, focused }) => {
      if (optedIn) {
        localStorage.setItem("diffpilot.notify-when-done", "on");
        sessionStorage.setItem("test-permission", "granted");
      }
      window.__notifications = [];
      class Recorder {
        static get permission() {
          return sessionStorage.getItem("test-permission") ?? "default";
        }
        static async requestPermission() {
          sessionStorage.setItem("test-permission", "granted");
          return "granted";
        }
        onclick: ((e: Event) => void) | null = null;
        constructor(
          public title: string,
          public options: NotificationOptions,
        ) {
          window.__notifications.push(this);
        }
        close() {}
      }
      Object.defineProperty(window, "Notification", { value: Recorder, configurable: true });
      if (!focused) document.hasFocus = () => false;
    },
    { optedIn, focused },
  );
}

test("Settings: turning the opt-in on asks the browser and is kept in this browser", async ({ page }) => {
  await recordNotifications(page, { optedIn: false, focused: true });
  await page.goto("/settings");
  const box = page.getByRole("checkbox", { name: "Notify me when work finishes" });
  await expect(box).not.toBeChecked();
  await box.check();
  await expect(box).toBeChecked();
  expect(await page.evaluate(() => sessionStorage.getItem("test-permission"))).toBe("granted");
  await page.reload();
  await expect(page.getByRole("checkbox", { name: "Notify me when work finishes" })).toBeChecked();
});

test("a tab in the background notifies of an analysed flow and an Ask answer; a click opens it", async ({
  page,
}) => {
  await recordNotifications(page, { optedIn: true, focused: false });
  const shown = () => page.evaluate(() => window.__notifications.map((n) => [n.title, n.options.body]));
  const nav = page.getByRole("navigation");

  await page.goto(`/analyses/${ANALYSIS_ID}?scenario=tab-status#f3`);
  await page.getByRole("main").getByRole("button", { name: "Analyse this flow" }).click();
  await page.clock.fastForward(1_100);
  await expect(page.getByText(/Being analysed · started 0 min ago/)).toBeVisible();
  await nav.getByRole("button", { name: /^Flow 1:/ }).click();
  await nav.getByRole("button", { name: /^Step 5:/ }).click();
  await page.clock.fastForward(71_000);
  await expect.poll(shown).toEqual([["Flow 3 analysed — NC#482", "The LLM analysis of flow 3 is ready."]]);
  await expect(page).not.toHaveURL(/#f3$/);
  await page.evaluate(() => window.__notifications[0]!.onclick!(new Event("click")));
  await expect(page).toHaveURL(/#f3$/);

  await nav.getByRole("button", { name: /^Flow 1:/ }).click();
  await nav.getByRole("button", { name: /^Step 5:/ }).click();
  await page.keyboard.press("a");
  await page
    .getByRole("dialog", { name: "Ask" })
    .getByRole("textbox", { name: "Your question" })
    .fill("Why retry here?");
  await page.keyboard.press("Enter");
  await page.clock.fastForward(1_000);
  await expect(page.getByRole("dialog", { name: "Ask" }).getByText("Why retry here?")).toBeVisible();
  await page.clock.fastForward(11_000);
  await page.clock.fastForward(5_000);
  await expect
    .poll(shown)
    .toContainEqual(["Answer ready — NC#482", "LLM answer to your question on flow 1, step 5."]);
  await nav.getByRole("button", { name: /^Flow 1:/ }).click();
  await expect(page).toHaveURL(/#f1$/);
  await page.evaluate(() => window.__notifications.at(-1)!.onclick!(new Event("click")));
  await expect(page).toHaveURL(/#f1\.5$/);
  expect(await shown()).toHaveLength(2);
});

test("in the Files view an Ask answer notifies too; a click opens its lines there", async ({ page }) => {
  await recordNotifications(page, { optedIn: true, focused: false });
  const shown = () => page.evaluate(() => window.__notifications.map((n) => [n.title, n.options.body]));
  await page.goto(
    `/analyses/${ANALYSIS_ID}/files?scenario=tab-status#lines?file=src%2Frpc%2Fclient.rs&side=head&line=98&end=101`,
  );
  const inspector = page.getByRole("complementary", { name: "Inspector" });
  await inspector.getByRole("button", { name: "Ask about these lines" }).click();
  const chat = page.getByRole("dialog", { name: "Ask" });
  await chat.getByRole("textbox", { name: "Your question" }).fill("Why sleep here?");
  await page.keyboard.press("Enter");
  await page.clock.fastForward(1_000);
  await expect(chat.getByText("Why sleep here?")).toBeVisible();
  await page.clock.fastForward(16_000);
  await expect
    .poll(shown)
    .toContainEqual(["Answer ready — NC#482", "LLM answer to your question on src/rpc/client.rs:98–101."]);
  await page
    .getByRole("navigation", { name: "Files" })
    .getByRole("button", { name: /lib\.rs/ })
    .click();
  await page.evaluate(() => window.__notifications.at(-1)!.onclick!(new Event("click")));
  await expect(page).toHaveURL(/\/files#lines\?file=src%2Frpc%2Fclient\.rs&side=head&line=98&end=101$/);
});
