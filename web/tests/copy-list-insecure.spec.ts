import { expect, test } from "@playwright/test";

// The LAN deployment (http://agentic:3000) is not a secure context: no Clipboard API there. This loads the real
// copyList module from the dev server into a page whose origin is plain HTTP on a LAN host name, clicks a button
// that copies, and reads the system clipboard back from a secure page of the same browser.

const LAN = "http://agentic.test:3000";

const HARNESS = `<!doctype html><html><body style="margin:0">
<div style="height:3000px"></div>
<button id="copy" type="button">Copy list</button>
<div style="height:3000px"></div>
<script>
  window.__vite_plugin_react_preamble_installed__ = true;
  window.$RefreshReg$ = () => {};
  window.$RefreshSig$ = () => (type) => type;
</script>
<script type="module">
  import { copyText, writeClipboard } from "/src/components/dashboard/common/copyList.ts";
  const lines = [
    {
      repo: { owner: "dashpay", name: "platform" },
      number: 123,
      url: "https://github.com/dashpay/platform/pull/123",
      title: "Title <b>",
      change_size: { size: "small", additions: 25, deletions: 3 },
      suffix: "waits on @x for 5 d",
    },
    { repo: { owner: "dashpay", name: "platform" }, number: 7, url: "https://github.com/dashpay/platform/pull/7", title: "Second" },
  ];
  const button = document.getElementById("copy");
  button.addEventListener("click", async () => {
    window.copied = await writeClipboard(copyText(lines));
    window.focusedAfter = document.activeElement?.id ?? null;
    window.leftovers = document.querySelectorAll("textarea").length;
  });
  window.harnessReady = true;
</script>
</body></html>`;

test("Copy list over plain HTTP on a LAN host writes Markdown text and linked HTML to the clipboard", async ({
  page,
  context,
}) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"], { origin: "http://localhost:5174" });
  await context.route(`${LAN}/**`, async (route) => {
    const url = new URL(route.request().url());
    if (url.pathname === "/harness.html") {
      await route.fulfill({ contentType: "text/html", body: HARNESS });
      return;
    }
    // Every module request goes to the Vite dev server, as the LAN server would serve the built app.
    const response = await route.fetch({ url: `http://localhost:5174${url.pathname}${url.search}` });
    await route.fulfill({ response });
  });
  await page.goto(`${LAN}/harness.html`);
  await page.waitForFunction(() => (window as unknown as { harnessReady?: boolean }).harnessReady === true);
  expect(await page.evaluate(() => window.isSecureContext)).toBe(false);
  expect(await page.evaluate(() => typeof navigator.clipboard)).toBe("undefined");

  await page.locator("#copy").scrollIntoViewIfNeeded();
  const scrollBefore = await page.evaluate(() => window.scrollY);
  await page.locator("#copy").click();
  await page.waitForFunction(() => (window as unknown as { copied?: boolean }).copied !== undefined);
  const outcome = await page.evaluate(() => {
    const w = window as unknown as { copied: boolean; focusedAfter: string | null; leftovers: number };
    return {
      copied: w.copied,
      focusedAfter: w.focusedAfter,
      leftovers: w.leftovers,
      scrollY: window.scrollY,
    };
  });
  expect(outcome).toEqual({ copied: true, focusedAfter: "copy", leftovers: 0, scrollY: scrollBefore });

  const reader = await context.newPage();
  await reader.goto("http://localhost:5174/");
  const clip = await reader.evaluate(async () => {
    const [item] = await navigator.clipboard.read();
    return {
      types: [...item!.types].sort(),
      text: await (await item!.getType("text/plain")).text(),
      html: await (await item!.getType("text/html")).text(),
    };
  });
  expect(clip.types).toEqual(["text/html", "text/plain"]);
  expect(clip.text).toBe(
    "[dashpay/platform#123](https://github.com/dashpay/platform/pull/123) Title <b> - small (+25 / -3) — waits on @x for 5 d\n" +
      "[dashpay/platform#7](https://github.com/dashpay/platform/pull/7) Second",
  );
  expect(clip.html).toContain(
    '<a href="https://github.com/dashpay/platform/pull/123">dashpay/platform#123</a> Title &lt;b&gt; - small (+25 / -3) — waits on @x for 5 d<br>',
  );
  expect(clip.html).toContain(
    '<a href="https://github.com/dashpay/platform/pull/7">dashpay/platform#7</a> Second',
  );
});
