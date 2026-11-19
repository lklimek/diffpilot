import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { Annotation, LinesResponse } from "../../api/generated/types";
import { CodeView } from "./CodeView";

function plain(from: number, texts: string[], extra: Partial<LinesResponse> = {}): LinesResponse {
  return {
    path: "src/a.rs",
    side: "head",
    from,
    to: from + texts.length - 1,
    total_lines: from + texts.length - 1,
    lines: texts,
    changed: [],
    deleted: [],
    moved: [],
    comments: { scope: "in_diff", line_thread_ranges: [] },
    ...extra,
  };
}

const remark = (id: string, start: number, end: number, type: Annotation["type"] = "note"): Annotation => ({
  id,
  type,
  basis: "inference",
  range: { start_line: start, end_line: end },
  was: type === "behaviour" ? "retries once" : "",
  is: type === "behaviour" ? "retries all" : "",
  text: type === "behaviour" ? null : "A retry may repeat the request.",
  evidence: [],
});

const rowOf = (el: Element) => Number(el.closest("[data-line]")!.getAttribute("data-line"));

describe("CodeView in the Files view (brief §4.5)", () => {
  it("draws one ◆ per LLM remark, on the remark's own first line; none on lines without one", () => {
    render(
      <CodeView
        lines={plain(1, ["a", "b", "c", "d"])}
        annotations={[remark("n1", 1, 2, "behaviour"), remark("n2", 1, 1), remark("n3", 2, 3)]}
        band="medium"
      />,
    );
    const marks = [...document.querySelectorAll("[data-mark=remark]")];
    expect(marks.map(rowOf)).toEqual([1, 1, 2]);
    expect(screen.getByRole("button", { name: "LLM remark at line 1: Behaviour" })).toBeVisible();
    expect(screen.getByRole("button", { name: "LLM remark at line 2: Note" })).toBeVisible();
    // One run, one bar, one label for overlapping remarks.
    expect(document.querySelectorAll("[data-highlight]")).toHaveLength(1);
  });

  it("names each remark's step and gives a run the highest rating of its remarks' steps", async () => {
    render(
      <CodeView
        lines={plain(1, ["a", "b"])}
        annotations={[remark("n1", 1, 2, "behaviour"), remark("n2", 1, 2, "consequence")]}
        bandOf={(a) => (a.id === "n1" ? "low" : "high")}
        remarkOwner={(a) => (a.id === "n1" ? "flow 1 · step 2" : "flow 4 · step 2")}
      />,
    );
    const run = document.querySelector("[data-highlight]")!;
    expect(run).toHaveAttribute("data-band", "high");
    expect(run).toHaveTextContent("LLM remarks: behaviour (flow 1 · step 2), consequence (flow 4 · step 2)");
    expect(
      screen.getByRole("button", { name: "LLM remark at line 1: Consequence (flow 4 · step 2)" }),
    ).toBeVisible();
    await userEvent.hover(screen.getByRole("button", { name: /Behaviour \(flow 1 · step 2\)/ }));
    expect(screen.getByRole("dialog")).toHaveTextContent("Was: retries once");
    expect(screen.getByRole("dialog")).not.toHaveTextContent("A retry may repeat");
  });

  it("highlights the selected lines; Shift+click on a line number extends the selection", async () => {
    const onLineAction = vi.fn();
    render(
      <CodeView
        lines={plain(10, ["a", "b", "c", "d"])}
        selection={{ from: 11, to: 12 }}
        onLineAction={onLineAction}
      />,
    );
    expect([...document.querySelectorAll("[data-line][data-selected-line]")].map((r) => rowOf(r))).toEqual([
      11, 12,
    ]);
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Comment on line 10" }));
    expect(onLineAction).toHaveBeenLastCalledWith(
      { path: "src/a.rs", side: "head", start_line: null, line: 10 },
      { extend: false },
    );
    await user.keyboard("{Shift>}");
    await user.click(screen.getByRole("button", { name: "Comment on line 13" }));
    await user.keyboard("{/Shift}");
    expect(onLineAction).toHaveBeenLastCalledWith(
      { path: "src/a.rs", side: "head", start_line: null, line: 13 },
      { extend: true },
    );
  });

  it("dragging over line numbers extends the selection", () => {
    const onLineAction = vi.fn();
    render(<CodeView lines={plain(1, ["a", "b", "c"])} selection={null} onLineAction={onLineAction} />);
    const number = (n: number) => screen.getByRole("button", { name: `Comment on line ${n}` });
    fireEvent.pointerOver(number(2), { buttons: 0 });
    expect(onLineAction).not.toHaveBeenCalled();
    fireEvent.pointerOver(number(3), { buttons: 1 });
    expect(onLineAction).toHaveBeenCalledWith(
      { path: "src/a.rs", side: "head", start_line: null, line: 3 },
      { extend: true },
    );
  });

  it("a click in a hunk's code selects that hunk; the selected hunk's lines are marked", async () => {
    const onSelectHunk = vi.fn();
    const hunkOf = (side: string, line: number) => (side === "head" && line >= 2 && line <= 3 ? "h1" : null);
    const { rerender } = render(
      <CodeView
        lines={plain(1, ["a", "b", "c", "d"], { changed: [2, 3] })}
        hunkOf={hunkOf}
        onSelectHunk={onSelectHunk}
      />,
    );
    await userEvent.click(screen.getByText("a"));
    expect(onSelectHunk).not.toHaveBeenCalled();
    await userEvent.click(screen.getByText("c"));
    expect(onSelectHunk).toHaveBeenCalledWith("h1");
    expect([...document.querySelectorAll("[data-hunk]")].map(rowOf)).toEqual([2, 3]);
    rerender(
      <CodeView
        lines={plain(1, ["a", "b", "c", "d"], { changed: [2, 3] })}
        hunkOf={hunkOf}
        selectedHunk="h1"
        onSelectHunk={onSelectHunk}
      />,
    );
    expect([...document.querySelectorAll("[data-hunk-selected]")].map(rowOf)).toEqual([2, 3]);
  });
});
