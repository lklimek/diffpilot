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

// The icon opens a popover on hover; a click pins it, and its labelled button does the asking.
const ask = async (name: string) => {
  await userEvent.click(screen.getByRole("button", { name }));
  await userEvent.click(screen.getByRole("button", { name: "Ask about this line" }));
};

const asks = () => screen.getAllByRole("button", { name: /^Ask the LLM about / });

describe("Ask the LLM about a line: the code view", () => {
  it("offers the action on every line next to the comment action, one tab stop, arrow keys between lines", async () => {
    const onAskLine = vi.fn();
    render(<CodeView lines={stepLines("f1.2")} onLineAction={() => {}} onAskLine={onAskLine} />);
    expect(asks()).toHaveLength(9 + 3);
    expect(asks().filter((b) => b.tabIndex === 0)).toHaveLength(1);
    expect(asks()[0]).not.toHaveAttribute("title");
    // Tab from a line's comment action reaches the same line's Ask action.
    act(() => screen.getByRole("button", { name: "Comment on line 89" }).focus());
    await userEvent.tab();
    expect(screen.getByRole("button", { name: "Ask the LLM about line 89" })).toHaveFocus();
    await userEvent.keyboard("{ArrowUp}");
    expect(screen.getByRole("button", { name: "Ask the LLM about removed line 99" })).toHaveFocus();
    // Enter pins the popover, Tab (past its Close) reaches its button, Enter asks and unpins.
    await userEvent.keyboard("{Enter}");
    expect(onAskLine).not.toHaveBeenCalled();
    await userEvent.tab();
    await userEvent.tab();
    expect(screen.getByRole("button", { name: "Ask about this line" })).toHaveFocus();
    await userEvent.keyboard("{Enter}");
    expect(onAskLine).toHaveBeenLastCalledWith({
      path: "src/rpc/client.rs",
      side: "base",
      start_line: null,
      line: 99,
    });
    await ask("Ask the LLM about line 92");
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
    await ask("Ask the LLM about line 91");
    expect(onAskLine).toHaveBeenLastCalledWith({
      path: "src/rpc/client.rs",
      side: "head",
      start_line: 90,
      line: 93,
    });
    // A line outside the selection: that line only.
    selection.setBaseAndExtent(code(90).firstChild ?? code(90), 0, code(93).firstChild ?? code(93), 1);
    await ask("Ask the LLM about line 95");
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
    await userEvent.keyboard("{ArrowDown}{Enter}{Tab}{Tab}{Enter}");
    expect(onAskLine).toHaveBeenLastCalledWith({
      path: "src/rpc/client.rs",
      side: "base",
      start_line: null,
      line: 54,
    });
    expect(asks().filter((b) => b.tabIndex === 0)).toEqual([
      screen.getByRole("button", { name: "Ask the LLM about removed line 54" }),
    ]);
    // The code keeps its text: the action's mark is drawn by CSS.
    expect(document.querySelector('[data-diff-line="added"] code')).toHaveTextContent(
      /^match send\(http\)\.await \{$/,
    );
  });

  it("asks about the selected lines of one side when the clicked line is in the selection", async () => {
    const onAskLine = vi.fn();
    const added: DiffResponse = {
      ...diff,
      hunks: [
        {
          header: "@@ -54,1 +97,4 @@",
          lines: [97, 98, 99, 100].map((n) => ({
            kind: "added" as const,
            old_line: null,
            new_line: n,
            text: `line ${n}`,
            eol: null,
          })),
        },
      ],
    };
    render(<ClassicDiff diff={added} onAskLine={onAskLine} />);
    const code = (text: string) => screen.getByText(text);
    window.getSelection()!.setBaseAndExtent(code("line 97").firstChild!, 0, code("line 99").firstChild!, 2);
    await ask("Ask the LLM about line 98");
    expect(onAskLine).toHaveBeenLastCalledWith({
      path: "src/rpc/client.rs",
      side: "head",
      start_line: 97,
      line: 99,
    });
  });

  it("offers no Ask action without its callback", () => {
    render(<ClassicDiff diff={diff} />);
    expect(screen.queryAllByRole("button", { name: /^Ask the LLM about / })).toHaveLength(0);
  });
});

describe("code-view icons: popup on hover, pinned by a click until the next click", () => {
  const onAskLine = vi.fn();
  const icon = () => screen.getByRole("button", { name: "Ask the LLM about line 92" });
  const popup = () => screen.queryByRole("dialog", { name: "Ask the LLM about line 92" });
  const renderIt = () => render(<CodeView lines={stepLines("f1.2")} onAskLine={onAskLine} />);

  it("shows the explanation on hover and on keyboard focus, never as a plain tooltip", async () => {
    renderIt();
    expect(popup()).toBeNull();
    await userEvent.hover(icon());
    expect(popup()).toBeVisible();
    expect(popup()).toHaveTextContent(/Nothing is sent until you press Ask/);
    expect(screen.queryByRole("tooltip")).toBeNull();
    // The action only appears once pinned.
    expect(screen.queryByRole("button", { name: "Ask about this line" })).toBeNull();
    await userEvent.unhover(icon());
    expect(popup()).toBeNull();
    // Keyboard focus (the last input was a key, not the pointer).
    await userEvent.keyboard("{ArrowDown}");
    act(() => icon().focus());
    expect(popup()).toBeVisible();
  });

  it("a click pins it until the next click anywhere; Escape closes it", async () => {
    renderIt();
    await userEvent.click(icon());
    await userEvent.unhover(icon());
    expect(popup()).toBeVisible();
    expect(screen.getByRole("button", { name: "Ask about this line" })).toBeVisible();
    await userEvent.click(document.body);
    expect(popup()).toBeNull();
    await userEvent.click(icon());
    await userEvent.keyboard("{Escape}");
    expect(popup()).toBeNull();
  });

  it("the action inside the popup asks and unpins", async () => {
    onAskLine.mockClear();
    renderIt();
    await userEvent.click(icon());
    await userEvent.click(screen.getByRole("button", { name: "Ask about this line" }));
    expect(onAskLine).toHaveBeenCalledTimes(1);
    expect(popup()).toBeNull();
  });
});
