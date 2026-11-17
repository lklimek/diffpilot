import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type {
  Annotation,
  LinesResponse,
  PendingComment,
  PendingReview,
  RiskBand,
  Tour,
} from "../../api/generated/types";
import { expectTrustWording, toursByState } from "../../__tests__/wording";
import { FIXTURE_STATES, linesFor, seedFor, type FixtureState } from "../../mocks/fixtures";
import { findStep } from "../../screens/tour/sequence";
import { CodeView, type CodeViewProps } from "./CodeView";
import type { Highlight, Token } from "./highlight";

function stepProps(stepId: string, state: FixtureState = "default") {
  const tour = seedFor(state, "tour") as Tour;
  const { step } = findStep(tour, stepId)!;
  const { file, side, range } = step.facts;
  const pending = seedFor(state, "pending-review") as PendingReview;
  return {
    lines: linesFor(state, file, side, range.start_line, range.end_line)!,
    annotations: step.llm.annotations,
    band: step.llm.assessment?.risk ?? null,
    kind: step.llm.kind,
    threads: tour.threads,
    comments: pending.comments,
  } satisfies CodeViewProps;
}

const lineNumbers = (root: ParentNode, selector: string) =>
  [...root.querySelectorAll(selector)].map((r) => Number(r.getAttribute("data-line")));
const rowOf = (el: Element) => Number(el.closest("[data-line]")!.getAttribute("data-line"));

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

function note(id: string, start: number, end: number, extra: Partial<Annotation> = {}): Annotation {
  return {
    id,
    type: "question",
    basis: "inference",
    range: { start_line: start, end_line: end },
    was: "one attempt",
    is: "three attempts",
    text: "Is a retry safe here?",
    evidence: [],
    ...extra,
  };
}

describe("CodeView", () => {
  it("renders numbered lines of the range in a labelled, keyboard-scrollable region", () => {
    render(<CodeView {...stepProps("f1.2")} />);
    const region = screen.getByRole("region", { name: "src/rpc/client.rs, lines 88 to 96" });
    expect(region).toHaveAttribute("tabindex", "0");
    expect(lineNumbers(region, "[data-line]:not([data-removed])")).toEqual([
      88, 89, 90, 91, 92, 93, 94, 95, 96,
    ]);
    expect(region).toHaveTextContent("impl RpcClient {");
  });

  it("puts a neutral gutter mark on every changed line, annotated or not", () => {
    render(<CodeView {...stepProps("f1.2")} />);
    expect(lineNumbers(document, "[data-line][data-changed]")).toEqual([89, 91, 92, 93, 94, 95, 96]);
    const marks = document.querySelectorAll("[data-mark=changed]");
    expect([...marks].map(rowOf)).toEqual([89, 91, 92, 93, 94, 95, 96]);
    for (const mark of marks) expect(mark).toHaveTextContent("changed line");
  });

  it("draws the LLM bar only on annotated ranges; changed lines without a remark stay uncoloured", () => {
    render(<CodeView {...stepProps("f1.2")} />);
    const highlights = document.querySelectorAll("[data-highlight]");
    expect(highlights).toHaveLength(1);
    expect(lineNumbers(highlights[0]!, "[data-line]")).toEqual([91, 92, 93, 94, 95]);
    // 89 and 96 are changed lines without an annotation.
    for (const n of [89, 96]) {
      expect(document.querySelector(`[data-line="${n}"]`)!.closest("[data-highlight]")).toBeNull();
    }
  });

  it.each<[RiskBand | null, string, string]>([
    ["high", "solid", "▲ HIGH"],
    ["medium", "dashed", "◆ MEDIUM"],
    ["low", "dotted", "● LOW"],
    [null, "dotted", "◇ not rated"],
  ])(
    "styles the bar by the step band %s (%s) with a label strip, never in the accent colour",
    (band, style, text) => {
      render(<CodeView lines={plain(1, ["a", "b"])} annotations={[note("x.a1", 2, 2)]} band={band} />);
      const highlight = document.querySelector("[data-highlight]")!;
      expect(highlight).toHaveAttribute("data-band", band ?? "none");
      expect(highlight.className).toContain(`border-${style}`);
      expect(highlight.className).not.toMatch(/accent/);
      expect(
        within(highlight as HTMLElement).getByText(`${text} · LLM remark: question`),
      ).toBeInTheDocument();
    },
  );

  it("keeps line numbers and code in the same columns inside and outside a highlight (UX-002)", () => {
    render(<CodeView {...stepProps("f1.2")} />);
    const segments = [...document.querySelectorAll("[data-segment]")];
    expect(segments.length).toBeGreaterThan(2);
    // Every segment reserves the same 5 px bar column; only colour and style differ.
    for (const segment of segments) expect(segment.className).toMatch(/(^|\s)border-l-5(\s|$)/);
  });

  it("exposes each highlight as a mark described by its was → is text", () => {
    render(<CodeView {...stepProps("f1.2")} />);
    const marks = screen.getAllByRole("mark");
    expect(marks).toHaveLength(1);
    expect(marks[0]).toHaveAttribute("data-highlight");
    expect(marks[0]).toHaveAccessibleDescription(/^Was: /);
  });

  it("states only was → is in the hover tooltip of a highlight", async () => {
    const props = stepProps("f1.5");
    render(<CodeView {...props} />);
    await userEvent.hover(document.querySelector("[data-highlight]")!);
    const tip = screen.getByRole("tooltip");
    for (const a of props.annotations) {
      expect(tip).toHaveTextContent(`Was: ${a.was}`);
      expect(tip).toHaveTextContent(`Is: ${a.is}`);
      if (a.text) expect(tip).not.toHaveTextContent(a.text);
    }
    expect(tip).not.toHaveTextContent(/HIGH|risk/i);
  });

  it("reaches a highlight by keyboard, shows its tooltip and selects its annotations", async () => {
    const onSelectAnnotation = vi.fn();
    const props = stepProps("f1.5");
    const { rerender } = render(<CodeView {...props} onSelectAnnotation={onSelectAnnotation} />);
    const label = screen.getByRole("button", { name: /HIGH · LLM remark/ });
    act(() => label.focus());
    expect(screen.getByRole("tooltip")).toBeVisible();
    await userEvent.keyboard("{Enter}");
    expect(onSelectAnnotation).toHaveBeenCalledWith(["f1.5.a1", "f1.5.a2", "f1.5.a3"]);
    rerender(<CodeView {...props} onSelectAnnotation={onSelectAnnotation} selectedAnnotation="f1.5.a2" />);
    expect(document.querySelector("[data-highlight]")).toHaveAttribute("data-selected");
  });

  it("marks the first line of a highlight with an LLM remark marker", () => {
    render(<CodeView {...stepProps("f1.2")} />);
    const remarks = document.querySelectorAll("[data-mark=remark]");
    expect([...remarks].map(rowOf)).toEqual([91]);
    expect(remarks[0]).toHaveTextContent("LLM remark");
  });

  it("shows ≠ on moved lines that differ from the moved original, inside a moved block", () => {
    render(<CodeView {...stepProps("f5.1")} />);
    expect(lineNumbers(document, "[data-line][data-moved]")).toHaveLength(58);
    const differing = document.querySelectorAll("[data-mark=differs]");
    expect([...differing].map(rowOf)).toEqual([49, 50]);
    expect(differing[0]).toHaveTextContent("≠");
    expect(differing[0]).toHaveTextContent("differs from the moved original");
  });

  it("marks an existing GitHub thread and the reviewer's own pending comment on their lines", () => {
    const { unmount } = render(<CodeView {...stepProps("f1.5")} />);
    const threads = document.querySelectorAll("[data-mark=thread]");
    expect([...threads].map(rowOf)).toEqual([99]);
    expect(threads[0]).toHaveTextContent(
      "GitHub thread by bob: Should we retry non-idempotent calls at all?",
    );
    unmount();
    render(<CodeView {...stepProps("f1.2", "missed")} />);
    const own = document.querySelectorAll("[data-mark=comment]");
    expect([...own].map(rowOf)).toEqual([96]);
    expect(own[0]).toHaveTextContent("your comment");
  });

  it("ignores threads and comments of other files, sides or without a line", () => {
    const comment: PendingComment = {
      id: "c",
      thread_id: "t",
      path: "src/other.rs",
      kind: "line",
      side: "head",
      start_line: null,
      line: 1,
      body: "x",
    };
    const tour = seedFor("default", "tour") as Tour;
    render(
      <CodeView
        lines={plain(1, ["a"], { path: "README.md" })}
        threads={tour.threads.map((t) => ({ ...t, line: 1 }))}
        comments={[comment, { ...comment, path: "README.md", side: "base" }, { ...comment, line: null }]}
      />,
    );
    // Only the README thread is on README.md, head line 1.
    expect(document.querySelectorAll("[data-mark=thread]")).toHaveLength(1);
    expect(document.querySelectorAll("[data-mark=comment]")).toHaveLength(0);
  });

  it("shows removed code inline at its marker by default, numbered at the base commit", () => {
    render(<CodeView {...stepProps("f1.2")} />);
    const blocks = screen.getAllByRole("group", { name: /^Removed code/ });
    expect(blocks.map((b) => b.getAttribute("aria-label"))).toEqual([
      "Removed code after line 88: 1 line from base line 99",
      "Removed code after line 90: 3 lines from base line 101",
    ]);
    expect(lineNumbers(blocks[1]!, "[data-line][data-removed]")).toEqual([101, 102, 103]);
    expect(within(blocks[0]!).getByText("/// Sends a request.", { exact: false })).toBeInTheDocument();
    const order = [...document.querySelectorAll("[data-line]")].map((r) => r.getAttribute("data-line"));
    expect(order.slice(0, 3)).toEqual(["88", "99", "89"]);
  });

  it("says how many deleted lines were cut and offers to load the base side", async () => {
    const onLoadDeleted = vi.fn();
    const block = { after_line: 1, base_start: 10, lines: ["x", "y"], total_lines: 7 };
    const lines = plain(1, ["a", "b"], { deleted: [block] });
    const { rerender } = render(<CodeView lines={lines} onLoadDeleted={onLoadDeleted} />);
    await userEvent.click(screen.getByRole("button", { name: "5 more deleted lines" }));
    expect(onLoadDeleted).toHaveBeenCalledWith(block);
    rerender(<CodeView lines={lines} />);
    expect(screen.queryByRole("button", { name: /more deleted lines/ })).toBeNull();
    expect(screen.getByText("5 more deleted lines")).toBeInTheDocument();
    rerender(<CodeView lines={plain(1, ["a"], { deleted: [{ ...block, lines: [], total_lines: 1 }] })} />);
    expect(screen.getByText("1 more deleted line")).toBeInTheDocument();
  });

  it("captions removed code read from the base commit", () => {
    render(<CodeView {...stepProps("f4.1")} />);
    const region = screen.getByRole("region", { name: /src\/rpc\/client.rs, lines 52 to 61, removed code/ });
    expect(within(region).getByText("Removed code at the base commit")).toBeInTheDocument();
    expect(region.querySelectorAll("[data-changed]")).toHaveLength(10);
  });

  it("gives a context step its own style hook and caption (UX-003)", () => {
    const { unmount } = render(<CodeView {...stepProps("f1.1")} />);
    const region = screen.getByRole("region", { name: /src\/mempool\/mod.rs/ });
    expect(region).toHaveAttribute("data-kind", "context");
    expect(within(region).getByText("Unchanged code connecting the flow")).toBeInTheDocument();
    unmount();
    render(<CodeView {...stepProps("f1.2")} />);
    expect(screen.getByRole("region", { name: /lines 88 to 96/ })).toHaveAttribute("data-kind", "modified");
    expect(screen.queryByText("Unchanged code connecting the flow")).toBeNull();
  });

  it("offers a keyboard-reachable comment action on every displayed line with one tab stop", async () => {
    const onLineAction = vi.fn();
    render(<CodeView {...stepProps("f1.2")} onLineAction={onLineAction} />);
    const buttons = screen.getAllByRole("button", { name: /^Comment on / });
    expect(buttons).toHaveLength(9 + 4);
    expect(buttons.filter((b) => b.tabIndex === 0)).toHaveLength(1);
    act(() => buttons[0]!.focus());
    await userEvent.keyboard("{ArrowDown}");
    expect(screen.getByRole("button", { name: "Comment on removed line 99" })).toHaveFocus();
    await userEvent.keyboard("{ArrowDown}{Enter}");
    expect(onLineAction).toHaveBeenLastCalledWith({
      path: "src/rpc/client.rs",
      side: "head",
      start_line: null,
      line: 89,
    });
    await userEvent.click(screen.getByRole("button", { name: "Comment on removed line 99" }));
    expect(onLineAction).toHaveBeenLastCalledWith({
      path: "src/rpc/client.rs",
      side: "base",
      start_line: null,
      line: 99,
    });
    await userEvent.keyboard("{End}");
    expect(screen.getByRole("button", { name: "Comment on line 96" })).toHaveFocus();
    expect(
      screen.getAllByRole("button", { name: /^Comment on / }).filter((b) => b.tabIndex === 0),
    ).toHaveLength(1);
  });

  it("names comment actions on removed code read from the base commit as removed lines", () => {
    render(<CodeView {...stepProps("f4.1")} onLineAction={() => {}} />);
    expect(screen.getAllByRole("button", { name: /^Comment on removed line \d+$/ })).toHaveLength(10);
  });

  it("renders line numbers as plain text without a line action", () => {
    render(<CodeView {...stepProps("f1.2")} />);
    expect(screen.queryAllByRole("button", { name: /^Comment on / })).toHaveLength(0);
  });

  it("highlights the line being commented on", () => {
    render(<CodeView {...stepProps("f1.2")} activeLine={{ side: "head", line: 92 }} />);
    const row = document.querySelector('[data-line="92"]')!;
    expect(row).toHaveAttribute("data-commenting");
    expect(row).toHaveTextContent("commenting on this line");
    expect(document.querySelectorAll("[data-commenting]")).toHaveLength(1);
  });

  it("renders code, LLM and GitHub text as text, never as markup (T12)", async () => {
    const evil = '<img src=x onerror="alert(1)"><a href="javascript:alert(1)">x</a>';
    const highlight: Highlight = async (_, lines) => lines.map((l) => [{ content: l, light: "#000000" }]);
    const thread = (seedFor("default", "tour") as Tour).threads[0]!;
    render(
      <CodeView
        lines={plain(1, [evil])}
        annotations={[note("x.a1", 1, 1, { was: evil, is: evil })]}
        threads={[{ ...thread, path: "src/a.rs", line: 1, author: evil, excerpt: evil }]}
        highlight={highlight}
      />,
    );
    await act(async () => {});
    expect(document.querySelector("[data-token]")).not.toBeNull();
    await userEvent.hover(document.querySelector("[data-highlight]")!);
    expect(document.querySelector("img")).toBeNull();
    expect(document.querySelector("a")).toBeNull();
    expect(screen.getByRole("tooltip")).toHaveTextContent(evil);
  });

  describe("syntax highlighting", () => {
    it("paints plain text first, then the tokens of the displayed lines only", async () => {
      let resolve: (tokens: Token[][]) => void = () => {};
      const highlight = vi.fn<Highlight>(() => new Promise((r) => (resolve = r)));
      render(<CodeView lines={plain(7, ["let a = 1;", "let b = 2;"])} highlight={highlight} />);
      expect(highlight).toHaveBeenCalledWith(
        "src/a.rs",
        ["let a = 1;", "let b = 2;"],
        expect.any(AbortSignal),
      );
      expect(document.querySelector("[data-line='7'] code")).toHaveTextContent("let a = 1;");
      expect(document.querySelector("[data-token]")).toBeNull();
      await act(async () =>
        resolve([
          [{ content: "let", light: "#cf222e", dark: "#ff7b72" }, { content: " a = 1;" }],
          [{ content: "let b = 2;" }],
        ]),
      );
      const token = document.querySelector("[data-line='7'] [data-token]") as HTMLElement;
      expect(token).toHaveTextContent("let");
      expect(token.style.getPropertyValue("--tok-light")).toBe("#cf222e");
      expect(token.style.getPropertyValue("--tok-dark")).toBe("#ff7b72");
      expect(document.querySelector("[data-line='7'] code")).toHaveTextContent("let a = 1;");
    });

    it("highlights removed code in its own call", () => {
      const highlight = vi.fn<Highlight>(async () => null);
      const block = { after_line: 7, base_start: 3, lines: ["old();"], total_lines: 1 };
      render(<CodeView lines={plain(7, ["new();"], { deleted: [block] })} highlight={highlight} />);
      expect(highlight).toHaveBeenCalledTimes(2);
      expect(highlight).toHaveBeenCalledWith("src/a.rs", ["new();"], expect.any(AbortSignal));
      expect(highlight).toHaveBeenCalledWith("src/a.rs", ["old();"], expect.any(AbortSignal));
    });

    it("drops colours that no longer match the path or highlighter (same lines reference)", async () => {
      const lines = plain(1, ["let a = 1;"], { path: "src/a.rs" });
      const colour: Highlight = async (_, l) => l.map((t) => [{ content: t, light: "#111111" }]);
      const { rerender } = render(<CodeView lines={lines} highlight={colour} />);
      await act(async () => {});
      expect(document.querySelector("[data-token]")).not.toBeNull();
      rerender(<CodeView lines={lines} />);
      expect(document.querySelector("[data-token]")).toBeNull();

      rerender(<CodeView lines={lines} highlight={colour} />);
      await act(async () => {});
      expect(document.querySelector("[data-token]")).not.toBeNull();
      const unsupported: Highlight = async (path, l) => (path.endsWith(".rs") ? colour(path, l) : null);
      rerender(
        <CodeView lines={{ ...lines, path: "LICENSE", lines: lines.lines }} highlight={unsupported} />,
      );
      await act(async () => {});
      expect(document.querySelector("[data-token]")).toBeNull();

      rerender(<CodeView lines={lines} highlight={colour} />);
      await act(async () => {});
      expect(document.querySelector("[data-token]")).not.toBeNull();
      rerender(<CodeView lines={lines} highlight={async () => Promise.reject(new Error("offline"))} />);
      await act(async () => {});
      expect(document.querySelector("[data-token]")).toBeNull();
    });

    it("aborts a running highlight when the lines change", async () => {
      const signals: AbortSignal[] = [];
      const highlight: Highlight = (_, __, signal) => {
        signals.push(signal!);
        return new Promise(() => {});
      };
      const { rerender } = render(<CodeView lines={plain(1, ["a"])} highlight={highlight} />);
      rerender(<CodeView lines={plain(1, ["b"])} highlight={highlight} />);
      expect(signals[0]!.aborted).toBe(true);
      expect(signals.at(-1)!.aborted).toBe(false);
    });

    it.each<[string, Highlight]>([
      ["no language", async () => null],
      ["a load failure", async () => Promise.reject(new Error("offline"))],
      ["tokens that do not match the text", async () => [[{ content: "other" }]]],
    ])("falls back to plain text on %s", async (_, highlight) => {
      render(<CodeView lines={plain(1, ["let a = 1;"])} highlight={highlight} />);
      await act(async () => {});
      expect(document.querySelector("[data-token]")).toBeNull();
      expect(document.querySelector("[data-line='1'] code")).toHaveTextContent("let a = 1;");
    });
  });

  it.each(FIXTURE_STATES)("keeps the trust wording in every step of state %s", (state) => {
    const tour = toursByState.find(([s]) => s === state)![1];
    for (const flow of tour.flows) {
      for (const step of flow.steps) {
        const { unmount } = render(<CodeView {...stepProps(step.id, state)} onLineAction={() => {}} />);
        expectTrustWording(document.body.textContent ?? "");
        unmount();
      }
    }
  });
});
