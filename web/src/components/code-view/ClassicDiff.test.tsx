import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { DiffResponse, PendingComment } from "../../api/generated/types";
import { ClassicDiff } from "./ClassicDiff";
import { createShikiHighlight, MAX_TOKENS, type Highlight, type ShikiModules } from "./highlight";

const diff: DiffResponse = {
  path: "src/rpc/client.rs",
  side: "head",
  hunks: [
    {
      header: "@@ -54,1 +97,2 @@ impl RpcClient",
      lines: [
        { kind: "context", old_line: 53, new_line: 96, text: "    let timeout = t;", eol: null },
        { kind: "removed", old_line: 54, new_line: null, text: "    send(http).await?;", eol: null },
        { kind: "added", old_line: null, new_line: 97, text: "    match send(http).await {", eol: null },
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
      "54−removed line    send(http).await?;",
      "97+added line    match send(http).await {",
    ]);
  });

  it("shows a change of only the line ending: CRLF and a missing final newline", () => {
    const endings: DiffResponse = {
      path: "a.txt",
      side: "head",
      hunks: [
        {
          header: "@@ -1,2 +1,2 @@",
          lines: [
            { kind: "removed", old_line: 1, new_line: null, text: "x", eol: "crlf" },
            { kind: "added", old_line: null, new_line: 1, text: "x", eol: null },
            { kind: "removed", old_line: 2, new_line: null, text: "last", eol: "no_newline" },
            { kind: "added", old_line: null, new_line: 2, text: "last", eol: null },
          ],
        },
      ],
    };
    render(<ClassicDiff diff={endings} />);
    const rows = [...document.querySelectorAll("[data-diff-line]")].map((r) => r.textContent);
    expect(rows).toEqual([
      "1−removed linex(CRLF)",
      "1+added linex",
      "2−removed linelast(no newline at end of file)",
      "2+added linelast",
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
        eol: null,
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

  it("marks the reviewer's pending comments on their side's line number and opens them in place", async () => {
    const comment: PendingComment = {
      id: "c1",
      thread_id: "t1",
      path: "src/rpc/client.rs",
      kind: "line",
      side: "head",
      start_line: null,
      line: 96,
      body: "Bounded?",
    };
    render(
      <ClassicDiff diff={diff} comments={[comment, { ...comment, id: "c2", side: "base", line: 54 }]} />,
    );
    const head = screen.getByRole("button", { name: "Your pending comment at line 96" });
    expect(head.closest("[data-diff-line]")).toHaveAttribute("data-diff-line", "context");
    const base = screen.getByRole("button", { name: "Your pending comment at removed line 54" });
    expect(base.closest("[data-diff-line]")).toHaveAttribute("data-diff-line", "removed");
    await userEvent.click(head);
    await userEvent.click(screen.getByRole("button", { name: "Open the comment here" }));
    const panel = screen.getByRole("list", { name: "Your pending comments at line 96" });
    expect(within(panel).getByText("Bounded?")).toBeVisible();
    // Hide in the panel's top-right corner closes it and gives the focus back to the line's marker.
    screen.getByRole("button", { name: "Hide your pending comment" }).focus();
    await userEvent.keyboard("{Enter}");
    expect(screen.queryByRole("list", { name: "Your pending comments at line 96" })).toBeNull();
    expect(head).toHaveFocus();
  });

  it("keeps a row with a 24 px comment marker aligned with its line numbers", () => {
    const comment: PendingComment = {
      id: "c1",
      thread_id: "t1",
      path: "src/rpc/client.rs",
      kind: "line",
      side: "head",
      start_line: null,
      line: 97,
      body: "Bounded?",
    };
    render(<ClassicDiff diff={diff} comments={[comment]} />);
    const row = screen
      .getByRole("button", { name: "Your pending comment at line 97" })
      .closest("[data-diff-line]")!;
    expect(row.className).toMatch(/(^|\s)min-h-6(\s|$)/);
    expect(row.className).toMatch(/(^|\s)items-center(\s|$)/);
  });
});
