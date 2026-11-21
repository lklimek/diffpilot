import { afterEach, describe, expect, it, vi } from "vitest";
import { copyText, writeClipboard, type CopyLine } from "./copyList";

const line = (over: Partial<CopyLine> = {}): CopyLine => ({
  repo: { owner: "dashpay", name: "platform" },
  number: 123,
  url: "https://github.com/dashpay/platform/pull/123",
  title: "Title",
  change_size: { size: "small", additions: 25, deletions: 3 },
  ...over,
});

describe("copyText", () => {
  it("writes the owner's Slack line: link, title, size with its counts, then the suffix", () => {
    expect(copyText([line({ suffix: "waits on @x for 5 d" })]).text).toBe(
      "[dashpay/platform#123](https://github.com/dashpay/platform/pull/123) Title - small (+25 / -3) — waits on @x for 5 d",
    );
  });

  it("leaves out an unknown size and an absent suffix, one line per PR", () => {
    const { text } = copyText([
      line({ change_size: undefined }),
      line({ number: 7, url: "https://x.test/7" }),
    ]);
    expect(text.split("\n")).toEqual([
      "[dashpay/platform#123](https://github.com/dashpay/platform/pull/123) Title",
      "[dashpay/platform#7](https://x.test/7) Title - small (+25 / -3)",
    ]);
  });

  it("carries the same lines as HTML anchors, escaping the title and the link", () => {
    const { html } = copyText([
      line({ title: `<b>"a" & b</b>`, url: "https://github.com/a?x=1&y=2", suffix: "waits on @x for 5 d" }),
      line({ change_size: null }),
    ]);
    expect(html).toBe(
      '<a href="https://github.com/a?x=1&amp;y=2">dashpay/platform#123</a> &lt;b&gt;&quot;a&quot; &amp; b&lt;/b&gt; - small (+25 / -3) — waits on @x for 5 d<br>' +
        '<a href="https://github.com/dashpay/platform/pull/123">dashpay/platform#123</a> Title',
    );
  });

  it("does not link a URL that is not http(s)", () => {
    const { text, html } = copyText([line({ url: "javascript:alert(1)", change_size: null })]);
    expect(text).toBe("dashpay/platform#123 Title");
    expect(html).toBe("dashpay/platform#123 Title");
  });
});

describe("writeClipboard", () => {
  const clip = { text: "plain", html: "<a>rich</a>" };
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
    Reflect.deleteProperty(navigator, "clipboard");
    Reflect.deleteProperty(document, "execCommand");
  });

  it("writes HTML and plain text through the Clipboard API in a secure context", async () => {
    const write = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { write }, configurable: true });
    vi.stubGlobal("isSecureContext", true);
    vi.stubGlobal(
      "ClipboardItem",
      class {
        constructor(public items: Record<string, Blob>) {}
      },
    );
    expect(await writeClipboard(clip)).toBe(true);
    const [[[item]]] = write.mock.calls as [[[{ items: Record<string, Blob> }]]];
    expect(Object.keys(item.items).sort()).toEqual(["text/html", "text/plain"]);
    expect(await item.items["text/html"]!.text()).toBe(clip.html);
    expect(await item.items["text/plain"]!.text()).toBe(clip.text);
  });

  it("over plain HTTP (no Clipboard API) copies both formats from a copy event and gives the focus back", async () => {
    vi.stubGlobal("isSecureContext", false);
    const button = document.createElement("button");
    document.body.append(button);
    button.focus();
    const data = new Map<string, string>();
    const execCommand = vi.fn((command: string) => {
      const event = new Event("copy", { cancelable: true }) as ClipboardEvent;
      Object.defineProperty(event, "clipboardData", {
        value: { setData: (type: string, value: string) => data.set(type, value) },
      });
      document.dispatchEvent(event);
      return command === "copy";
    });
    Object.defineProperty(document, "execCommand", { value: execCommand, configurable: true });
    expect(await writeClipboard(clip)).toBe(true);
    expect(Object.fromEntries(data)).toEqual({ "text/plain": "plain", "text/html": "<a>rich</a>" });
    expect(button).toHaveFocus();
    expect(document.querySelector("textarea")).toBeNull();
    button.remove();
  });

  it("falls back to the copy event when the Clipboard API refuses", async () => {
    vi.stubGlobal("isSecureContext", true);
    vi.stubGlobal("ClipboardItem", class {});
    Object.defineProperty(navigator, "clipboard", {
      value: { write: vi.fn().mockRejectedValue(new Error("denied")) },
      configurable: true,
    });
    const execCommand = vi.fn(() => true);
    Object.defineProperty(document, "execCommand", { value: execCommand, configurable: true });
    expect(await writeClipboard(clip)).toBe(true);
    expect(execCommand).toHaveBeenCalledWith("copy");
  });

  it("reports failure when nothing can copy", async () => {
    vi.stubGlobal("isSecureContext", false);
    Object.defineProperty(document, "execCommand", { value: () => false, configurable: true });
    expect(await writeClipboard(clip)).toBe(false);
    Reflect.deleteProperty(document, "execCommand");
    expect(await writeClipboard(clip)).toBe(false);
  });
});
