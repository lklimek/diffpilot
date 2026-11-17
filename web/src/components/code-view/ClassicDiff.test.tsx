import { act, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { DiffResponse } from "../../api/generated/types";
import { ClassicDiff } from "./ClassicDiff";
import { createShikiHighlight, MAX_TOKENS, type Highlight, type ShikiModules } from "./highlight";

const diff: DiffResponse = {
  path: "src/rpc/client.rs",
  side: "head",
  hunks: [
    {
      header: "@@ -54,1 +97,2 @@ impl RpcClient",
      lines: [
        { kind: "context", old_line: 53, new_line: 96, text: "    let timeout = t;" },
        { kind: "removed", old_line: 54, new_line: null, text: "    send(http).await?;" },
        { kind: "added", old_line: null, new_line: 97, text: "    match send(http).await {" },
      ],
    },
  ],
};

describe("ClassicDiff", () => {
  it("shows each hunk with its header and old / new line numbers, removed and added lines named", () => {
    render(<ClassicDiff diff={diff} />);
    const region = screen.getByRole("region", { name: "Classic diff of src/rpc/client.rs" });
    expect(region).toHaveTextContent("@@ -54,1 +97,2 @@ impl RpcClient");
    const rows = [...region.querySelectorAll("[data-diff-line]")];
    expect(rows.map((r) => r.getAttribute("data-diff-line"))).toEqual(["context", "removed", "added"]);
    expect(rows.map((r) => r.textContent)).toEqual([
      "5396 unchanged    let timeout = t;",
      "54−removed    send(http).await?;",
      "97+added    match send(http).await {",
    ]);
  });

  it("says so when the range has no change", () => {
    render(<ClassicDiff diff={{ ...diff, hunks: [] }} />);
    expect(screen.getByText("No change in this range.")).toBeInTheDocument();
  });

  it("colours all hunks with one highlight call, one fragment per hunk", async () => {
    const highlight = vi.fn<Highlight>(async (_, fragments) =>
      fragments.map((lines) => lines.map((l) => [{ content: l, light: "#111111" }])),
    );
    const two = { ...diff, hunks: [diff.hunks[0]!, { ...diff.hunks[0]!, header: "@@ -90 +130 @@" }] };
    render(<ClassicDiff diff={two} highlight={highlight} />);
    await act(async () => {});
    expect(highlight).toHaveBeenCalledTimes(1);
    const texts = diff.hunks[0]!.lines.map((l) => l.text);
    expect(highlight).toHaveBeenCalledWith("src/rpc/client.rs", [texts, texts], expect.any(AbortSignal));
    expect(document.querySelectorAll("[data-token]")).toHaveLength(6);
  });

  it("keeps one budget for all hunks: many small hunks of pathological lines (CWE-770)", async () => {
    const real = createShikiHighlight({
      core: () => import("shiki/core"),
      engine: () => import("shiki/engine/javascript"),
      light: () => import("shiki/themes/github-light-default.mjs"),
      dark: () => import("shiki/themes/github-dark-default.mjs"),
      langs: { rust: () => import("shiki/langs/rust.mjs") } as ShikiModules["langs"],
    });
    await real("w.rs", [["fn a() {}"]]);
    const highlight = vi.fn(real);
    const text = `${"a+".repeat(999)}a`;
    const hunks = Array.from({ length: 40 }, (_, h) => ({
      header: `@@ -${h * 5 + 1},5 +0,0 @@`,
      lines: Array.from({ length: 5 }, (_, i) => ({
        kind: "removed" as const,
        old_line: h * 5 + i + 1,
        new_line: null,
        text,
      })),
    }));
    render(<ClassicDiff diff={{ ...diff, hunks }} highlight={highlight} />);
    expect(highlight).toHaveBeenCalledTimes(1);
    let last = performance.now();
    let longest = 0;
    let running = true;
    const tick = () => {
      const now = performance.now();
      longest = Math.max(longest, now - last);
      last = now;
      if (running) setTimeout(tick, 0);
    };
    setTimeout(tick, 0);
    await highlight.mock.results[0]!.value;
    running = false;
    expect(longest).toBeLessThan(100);
    await act(async () => {});
    expect(document.querySelectorAll("[data-token]").length).toBeLessThanOrEqual(MAX_TOKENS);
  }, 30_000);
});
