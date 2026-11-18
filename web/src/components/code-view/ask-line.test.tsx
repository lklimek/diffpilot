import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { DiffResponse, Tour } from "../../api/generated/types";
import { linesFor, seedFor } from "../../mocks/fixtures";
import { findStep } from "../../screens/tour/sequence";
import { ClassicDiff } from "./ClassicDiff";
import { CodeView } from "./CodeView";

function stepLines(stepId: string) {
  const tour = seedFor("default", "tour") as Tour;
  const { file, side, range } = findStep(tour, stepId)!.step.facts;
  return linesFor("default", file, side, range.start_line, range.end_line)!;
}

const asks = () => screen.getAllByRole("button", { name: /^Ask the LLM about / });

describe("Ask the LLM about a line: the code view", () => {
  it("offers the action on every line next to the comment action, one tab stop, arrow keys between lines", async () => {
    const onAskLine = vi.fn();
    render(<CodeView lines={stepLines("f1.2")} onLineAction={() => {}} onAskLine={onAskLine} />);
    expect(asks()).toHaveLength(9 + 3);
    expect(asks().filter((b) => b.tabIndex === 0)).toHaveLength(1);
    expect(asks()[0]).toHaveAttribute("title", expect.stringMatching(/^Ask the LLM about line \d+/));
    // Tab from a line's comment action reaches the same line's Ask action.
    act(() => screen.getByRole("button", { name: "Comment on line 89" }).focus());
    await userEvent.tab();
    expect(screen.getByRole("button", { name: "Ask the LLM about line 89" })).toHaveFocus();
    await userEvent.keyboard("{ArrowUp}");
    expect(screen.getByRole("button", { name: "Ask the LLM about removed line 99" })).toHaveFocus();
    await userEvent.keyboard("{Enter}");
    expect(onAskLine).toHaveBeenLastCalledWith({
      path: "src/rpc/client.rs",
      side: "base",
      start_line: null,
      line: 99,
    });
    await userEvent.click(screen.getByRole("button", { name: "Ask the LLM about line 92" }));
    expect(onAskLine).toHaveBeenLastCalledWith({
      path: "src/rpc/client.rs",
      side: "head",
      start_line: null,
      line: 92,
    });
  });

  it("asks about the selected lines when the clicked line is in a selection over several lines", async () => {
    const onAskLine = vi.fn();
    render(<CodeView lines={stepLines("f1.2")} onAskLine={onAskLine} />);
    const code = (line: number) => document.querySelector(`[data-side="head"][data-line="${line}"] code`)!;
    const selection = window.getSelection()!;
    selection.setBaseAndExtent(code(90).firstChild ?? code(90), 0, code(93).firstChild ?? code(93), 1);
    await userEvent.click(screen.getByRole("button", { name: "Ask the LLM about line 91" }));
    expect(onAskLine).toHaveBeenLastCalledWith({
      path: "src/rpc/client.rs",
      side: "head",
      start_line: 90,
      line: 93,
    });
    // A line outside the selection: that line only.
    selection.setBaseAndExtent(code(90).firstChild ?? code(90), 0, code(93).firstChild ?? code(93), 1);
    await userEvent.click(screen.getByRole("button", { name: "Ask the LLM about line 95" }));
    expect(onAskLine).toHaveBeenLastCalledWith({
      path: "src/rpc/client.rs",
      side: "head",
      start_line: null,
      line: 95,
    });
  });

  it("offers no Ask action without its callback", () => {
    render(<CodeView lines={stepLines("f1.2")} onLineAction={() => {}} />);
    expect(screen.queryAllByRole("button", { name: /^Ask the LLM about / })).toHaveLength(0);
  });
});

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

describe("Ask the LLM about a line: the classic diff", () => {
  it("offers the action on every line: new lines at the head, removed lines at the base", async () => {
    const onAskLine = vi.fn();
    render(<ClassicDiff diff={diff} onAskLine={onAskLine} />);
    expect(asks().map((b) => b.getAttribute("aria-label"))).toEqual([
      "Ask the LLM about line 96",
      "Ask the LLM about removed line 54",
      "Ask the LLM about line 97",
    ]);
    expect(asks().filter((b) => b.tabIndex === 0)).toHaveLength(1);
    act(() => asks()[0]!.focus());
    await userEvent.keyboard("{ArrowDown}{Enter}");
    expect(onAskLine).toHaveBeenLastCalledWith({
      path: "src/rpc/client.rs",
      side: "base",
      start_line: null,
      line: 54,
    });
    expect(asks().filter((b) => b.tabIndex === 0)).toEqual([
      screen.getByRole("button", { name: "Ask the LLM about removed line 54" }),
    ]);
    // The rows keep their text: the action's mark is drawn by CSS.
    expect(document.querySelector('[data-diff-line="added"]')).toHaveTextContent(/^97\+added line/);
  });

  it("offers no Ask action without its callback", () => {
    render(<ClassicDiff diff={diff} />);
    expect(screen.queryAllByRole("button", { name: /^Ask the LLM about / })).toHaveLength(0);
  });
});
