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
    await user.click(screen.getByRole("button", { name: "Select line 10" }));
    expect(onLineAction).toHaveBeenLastCalledWith(
      { path: "src/a.rs", side: "head", start_line: null, line: 10 },
      { extend: false },
    );
    await user.keyboard("{Shift>}");
    await user.click(screen.getByRole("button", { name: "Select line 13" }));
    await user.keyboard("{/Shift}");
    expect(onLineAction).toHaveBeenLastCalledWith(
      { path: "src/a.rs", side: "head", start_line: null, line: 13 },
      { extend: true },
    );
  });

  it("dragging over line numbers extends the selection", () => {
    const onLineAction = vi.fn();
    render(<CodeView lines={plain(1, ["a", "b", "c"])} selection={null} onLineAction={onLineAction} />);
    const number = (n: number) => screen.getByRole("button", { name: `Select line ${n}` });
    fireEvent.pointerDown(number(1), { buttons: 1 });
    fireEvent.pointerOver(number(2), { buttons: 0 });
    expect(onLineAction).toHaveBeenCalledTimes(1);
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

  it("draws remarks on removed code: their bar and one ◆ on the first removed line they cover", () => {
    const lines = plain(10, ["a", "b"], {
      changed: [11],
      deleted: [{ after_line: 10, base_start: 20, lines: ["x", "y", "z"], total_lines: 3 }],
    });
    render(
      <CodeView lines={lines} removedAnnotations={[remark("r1", 21, 22, "consequence")]} band="medium" />,
    );
    const marks = [...document.querySelectorAll("[data-mark=remark]")];
    expect(marks).toHaveLength(1);
    expect(marks[0]!.closest("[data-line]")).toHaveAttribute("data-side", "base");
    expect(rowOf(marks[0]!)).toBe(21);
    expect(document.querySelectorAll("[data-highlight]")).toHaveLength(1);
  });

  it("with markers at the start only, a remark that starts above the shown lines gets no ◆ here", () => {
    render(
      <CodeView
        lines={plain(10, ["a", "b"])}
        annotations={[remark("n1", 5, 11)]}
        remarkStartOnly
        band="low"
      />,
    );
    expect(document.querySelectorAll("[data-mark=remark]")).toHaveLength(0);
    expect(document.querySelectorAll("[data-highlight]")).toHaveLength(1);
  });

  it("marks a selection of removed lines, keeps change colours under a selection, and outlines it", () => {
    const lines = plain(10, ["a", "b", "c"], {
      changed: [11],
      deleted: [{ after_line: 10, base_start: 20, lines: ["x", "y"], total_lines: 2 }],
    });
    const { rerender } = render(
      <CodeView lines={lines} selection={{ side: "base", from: 20, to: 21 }} onLineAction={() => {}} />,
    );
    const selected = [...document.querySelectorAll("[data-selected-line]")];
    expect(selected.map((r) => `${r.getAttribute("data-side")}:${r.getAttribute("data-line")}`)).toEqual([
      "base:20",
      "base:21",
    ]);
    rerender(<CodeView lines={lines} selection={{ from: 11, to: 11 }} onLineAction={() => {}} />);
    const row = document.querySelector('[data-line="11"][data-side="head"]') as HTMLElement;
    expect(row.className).toContain("bg-ad-bg");
    expect(row.style.boxShadow).toContain("var(--color-accent)");
  });

  it("marks where hunks start and end, so adjacent hunks are told apart", () => {
    const lines = plain(1, ["a", "b", "c", "d"], { changed: [1, 2, 3, 4] });
    render(
      <CodeView
        lines={lines}
        hunkOf={(_, line) => (line <= 2 ? "h1" : "h2")}
        selectedHunk="h2"
        onSelectHunk={() => {}}
      />,
    );
    const at = (n: number) => document.querySelector(`[data-line="${n}"]`)!;
    expect(at(1)).toHaveAttribute("data-hunk-start");
    expect(at(2)).toHaveAttribute("data-hunk-end");
    expect(at(3)).toHaveAttribute("data-hunk-start");
    expect((at(3) as HTMLElement).style.boxShadow).toContain("var(--color-accent)");
    expect((at(1) as HTMLElement).style.boxShadow).not.toContain("var(--color-accent)");
  });

  it("drags a selection only from a line number: a text drag passing over the numbers does not select", () => {
    const onLineAction = vi.fn();
    render(<CodeView lines={plain(10, ["a", "b", "c"])} selection={null} onLineAction={onLineAction} />);
    fireEvent.pointerOver(screen.getByRole("button", { name: "Select line 11" }), { buttons: 1 });
    expect(onLineAction).not.toHaveBeenCalled();
    fireEvent.pointerDown(screen.getByRole("button", { name: "Select line 10" }), { buttons: 1 });
    fireEvent.pointerOver(screen.getByRole("button", { name: "Select line 12" }), { buttons: 1 });
    expect(onLineAction).toHaveBeenLastCalledWith(expect.objectContaining({ line: 12 }), { extend: true });
  });

  it("the line's Ask popup can describe what the view does", async () => {
    render(
      <CodeView
        lines={plain(10, ["a"])}
        onAskLine={() => {}}
        askLineHint="Selects the line and opens Ask in the Inspector; the answer is LLM output."
      />,
    );
    await userEvent.hover(screen.getByRole("button", { name: "Ask the LLM about line 10" }));
    expect(await screen.findByRole("dialog")).toHaveTextContent(
      "Selects the line and opens Ask in the Inspector",
    );
  });
});
