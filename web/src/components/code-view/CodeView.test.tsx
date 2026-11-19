import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type {
  Annotation,
  LinesResponse,
  PendingComment,
  PendingReview,
  FlagLevel,
  RiskBand,
  Tour,
} from "../../api/generated/types";
import { expectTrustWording, toursByState } from "../../__tests__/wording";
import { FIXTURE_STATES, linesFor, seedFor, type FixtureState } from "../../mocks/fixtures";
import { findStep } from "../../screens/tour/sequence";
import { CodeView, type CodeViewProps } from "./CodeView";
import { createShikiHighlight, MAX_TOKENS, type Highlight, type ShikiModules, type Token } from "./highlight";

// Longest stretch without an event-loop turn while `work` runs: a 0 ms timer that re-arms itself.
async function longestSlice(work: () => Promise<unknown>): Promise<number> {
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
  await work();
  running = false;
  tick();
  return longest;
}

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
    type: "note",
    basis: "inference",
    range: { start_line: start, end_line: end },
    was: "",
    is: "",
    text: "A retry may repeat the request.",
    evidence: [],
    ...extra,
  };
}

describe("CodeView", () => {
  it("does not display empty was/is for a non-behaviour remark", () => {
    render(
      <CodeView
        lines={plain(1, ["send();"])}
        annotations={[note("n", 1, 1, { was: "", is: "" })]}
        band="low"
      />,
    );
    const label = screen.getByText(/LOW · LLM remark: note/);
    act(() => label.focus());
    expect(screen.queryByRole("tooltip")).toBeNull();
    expect(label).not.toHaveAccessibleDescription(/Was:/);
    expect(label).toHaveAccessibleDescription("No behaviour annotation on these lines.");
  });

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
    // 95 only moved into the new `loop` (whitespace only): unchanged code, no mark.
    expect(lineNumbers(document, "[data-line][data-changed]")).toEqual([89, 91, 92, 93, 94, 96]);
    const marks = document.querySelectorAll("[data-mark=changed]");
    expect([...marks].map(rowOf)).toEqual([89, 91, 92, 93, 94, 96]);
    for (const mark of marks) expect(mark).toHaveTextContent("added line");
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

  it.each<[RiskBand, string, string]>([
    ["high", "solid", "▲ HIGH"],
    ["medium", "dashed", "◆ MEDIUM"],
    ["low", "dotted", "● LOW"],
  ])(
    "styles the bar by the step band %s (%s) with a label strip, never in the accent colour",
    (band, style, text) => {
      render(<CodeView lines={plain(1, ["a", "b"])} annotations={[note("x.a1", 2, 2)]} band={band} />);
      const highlight = document.querySelector("[data-highlight]")!;
      expect(highlight).toHaveAttribute("data-band", band);
      expect(highlight).toHaveAttribute("data-bar", style);
      expect(highlight.className).toContain(`border-${style}`);
      expect(highlight.className).not.toMatch(/accent/);
      expect(within(highlight as HTMLElement).getByText(`${text} · LLM remark: note`)).toBeInTheDocument();
    },
  );

  it.each<[FlagLevel, RiskBand | null, string, string]>([
    ["red", "medium", "border-high-bd", "border-dashed"],
    ["yellow", "high", "border-med-bd", "border-solid"],
    ["red", "high", "border-high-bd", "border-solid"],
  ])(
    "draws a range with a %s flag in the flag's colour, keeping the rating's pattern (%s), and names the flag",
    (level, band, colour, pattern) => {
      const flag = note("x.a2", 1, 2, {
        type: "flag",
        text: "Potential double send:",
        flag: { level, scenario: ["A sends.", "B sends again."], not_checked: "Not run." },
      });
      render(<CodeView lines={plain(1, ["a", "b"])} annotations={[note("x.a1", 2, 2), flag]} band={band} />);
      const highlight = document.querySelector("[data-highlight]")!;
      expect(highlight).toHaveAttribute("data-flag", level);
      expect(highlight.className).toContain(colour);
      expect(highlight.className).toContain(pattern);
      const name = { red: "⚑ Red flag", yellow: "⚑ Yellow flag", unknown: "⚑ Flag" }[level];
      expect(within(highlight as HTMLElement).getByText(new RegExp(`^${name} · `))).toBeInTheDocument();
    },
  );

  it.each<[FlagLevel, string]>([
    ["red", "--color-high-bd"],
    ["unknown", "--color-mark"],
  ])(
    "draws a %s flag on an unrated step as the thin line in the flag's colour, not a full bar",
    (level, colour) => {
      const flag = note("x.a2", 1, 2, {
        type: "flag",
        text: "Potential double send:",
        flag: { level, scenario: ["A sends twice.", "B pays twice."], not_checked: "Not run." },
      });
      render(<CodeView lines={plain(1, ["a", "b"])} annotations={[flag]} band={null} />);
      const highlight = document.querySelector("[data-highlight]")!;
      expect(highlight).toHaveAttribute("data-flag", level);
      expect(highlight).toHaveAttribute("data-bar", "thin");
      expect(highlight.className).toContain("border-transparent");
      expect(highlight.className).toContain(`var(${colour})`);
    },
  );

  it("draws an unrated remark with a thin neutral line, not the LOW pattern, and says so in its description and popover", () => {
    render(<CodeView lines={plain(1, ["a", "b"])} annotations={[note("x.a1", 2, 2)]} band={null} />);
    const highlight = document.querySelector("[data-highlight]")!;
    expect(highlight).toHaveAttribute("data-band", "none");
    expect(highlight).toHaveAttribute("data-bar", "thin");
    expect(highlight.className).not.toMatch(/border-(dotted|dashed|double)|accent/);
    const label = within(highlight as HTMLElement).getByText("LLM remark: note");
    expect(label).toHaveTextContent(/^LLM remark: note$/);
    expect(highlight).not.toHaveTextContent("◇");
    expect(label).toHaveAccessibleDescription(/Risk not rated by the LLM/);
    act(() => screen.getByRole("button", { name: /^LLM remark at line 2:/ }).focus());
    expect(screen.getByRole("dialog", { name: /^LLM remark at line 2:/ })).toHaveTextContent(
      "Risk not rated by the LLM.",
    );
  });

  it("keeps line numbers and code in the same columns inside and outside a highlight", () => {
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
    expect(marks[0]).toHaveAccessibleDescription(/Was: /);
  });

  it("shows no tooltip on hovering or focusing a highlight; its was → is stays in the description", async () => {
    const props = stepProps("f1.5");
    render(<CodeView {...props} />);
    await userEvent.hover(document.querySelector("[data-highlight]")!);
    expect(screen.queryByRole("tooltip")).toBeNull();
    const label = screen.getByText(/HIGH · LLM remark/);
    act(() => label.focus());
    expect(screen.queryByRole("tooltip")).toBeNull();
    for (const a of props.annotations.filter((annotation) => annotation.type === "behaviour")) {
      expect(label.getAttribute("aria-describedby")).toBeTruthy();
      expect(label).toHaveAccessibleDescription(expect.stringContaining(`Was: ${a.was}`));
      expect(label).toHaveAccessibleDescription(expect.stringContaining(`Is: ${a.is}`));
    }
  });

  it("reaches a highlight by keyboard and selects its annotations", async () => {
    const onSelectAnnotation = vi.fn();
    const props = stepProps("f1.5");
    const { rerender } = render(<CodeView {...props} onSelectAnnotation={onSelectAnnotation} />);
    const label = screen.getByRole("button", { name: /HIGH · LLM remark/ });
    act(() => label.focus());
    expect(screen.queryByRole("tooltip")).toBeNull();
    await userEvent.keyboard("{Enter}");
    expect(onSelectAnnotation).toHaveBeenCalledWith(["f1.5.a1", "f1.5.a2", "f1.5.a3"]);
    rerender(<CodeView {...props} onSelectAnnotation={onSelectAnnotation} selectedAnnotation="f1.5.a2" />);
    expect(document.querySelector("[data-highlight]")).toHaveAttribute("data-selected");
  });

  it("marks the first line of each LLM remark with its own marker (one ◆ per remark)", () => {
    render(<CodeView {...stepProps("f1.2")} />);
    const remarks = document.querySelectorAll("[data-mark=remark]");
    expect([...remarks].map(rowOf)).toEqual([91, 93]);
    expect(remarks[0]).toHaveAccessibleName(/^LLM remark at line 91: /);
  });

  it("shows ≠ on moved lines that differ from the moved original, inside a moved block", () => {
    render(<CodeView {...stepProps("f5.1")} />);
    expect(lineNumbers(document, "[data-line][data-moved]")).toHaveLength(58);
    const differing = document.querySelectorAll("[data-mark=differs]");
    expect([...differing].map(rowOf)).toEqual([49, 50]);
    expect(differing[0]).toHaveTextContent("≠");
    expect(differing[0]).toHaveTextContent("differs from the moved original");
  });

  it("fills moved changed lines like the classic diff (green), the double bar marks them as moved", () => {
    render(<CodeView {...stepProps("f5.1")} />);
    const moved = [...document.querySelectorAll<HTMLElement>("[data-line][data-moved]")];
    expect(moved.some((el) => el.className.includes("bg-mv-bg"))).toBe(false);
    for (const el of moved.filter((m) => m.hasAttribute("data-changed")))
      expect(el.className).toMatch(/(^|\s)bg-ad-bg(\s|$)/);
  });

  it("marks an existing GitHub thread and the reviewer's own pending comment on their lines", () => {
    const { unmount } = render(<CodeView {...stepProps("f1.5")} />);
    const threads = document.querySelectorAll("[data-mark=thread]");
    expect([...threads].map(rowOf)).toEqual([99]);
    expect(threads[0]).toHaveAccessibleName("GitHub thread by bob at line 99");
    expect(threads[0]).toHaveAccessibleDescription(/GitHub review thread/);
    act(() => (threads[0] as HTMLElement).focus());
    expect(threads[0]).toHaveAccessibleDescription(/Should we retry non-idempotent calls at all\?/);
    unmount();
    render(<CodeView {...stepProps("f1.2", "missed")} />);
    const own = document.querySelectorAll("[data-mark=comment]");
    expect([...own].map(rowOf)).toEqual([96]);
    expect(screen.getByRole("button", { name: "Your pending comment at line 96" })).toBe(own[0]);
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
      "Removed code after line 95: 2 lines from base line 102",
    ]);
    expect(lineNumbers(blocks[1]!, "[data-line][data-removed]")).toEqual([102, 103]);
    expect(within(blocks[0]!).getByText("/// Sends a request.", { exact: false })).toBeInTheDocument();
    const order = [...document.querySelectorAll("[data-line]")].map((r) => r.getAttribute("data-line"));
    expect(order.slice(0, 3)).toEqual(["88", "99", "89"]);
  });

  it("says how many deleted lines were cut and offers to load the base side", async () => {
    const onLoadDeleted = vi.fn();
    const block = { after_line: 1, base_start: 10, lines: ["x", "y"], total_lines: 7 };
    const lines = plain(1, ["a", "b"], { deleted: [block] });
    const { rerender } = render(<CodeView lines={lines} onLoadDeleted={onLoadDeleted} />);
    // An icon: hover shows its popup, one click acts (no pin step, no second button).
    const icon = screen.getByRole("button", { name: "Show 5 more deleted lines" });
    expect(icon).not.toHaveAttribute("title");
    await userEvent.hover(icon);
    expect(screen.getByRole("dialog", { name: "5 more deleted lines" })).toBeVisible();
    expect(screen.queryByRole("button", { name: "Show the rest" })).toBeNull();
    await userEvent.click(icon);
    expect(onLoadDeleted).toHaveBeenCalledTimes(1);
    expect(onLoadDeleted).toHaveBeenCalledWith(block);
    expect(screen.queryByRole("dialog")).toBeNull();
    // Enter and Space act too.
    icon.focus();
    await userEvent.keyboard("{Enter}");
    await userEvent.keyboard(" ");
    expect(onLoadDeleted).toHaveBeenCalledTimes(3);
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

  it("gives a context step a style hook and a caption from the diff only when no line changed", () => {
    const caption = "Unchanged code: no changed lines in this range (from the diff).";
    const { unmount } = render(<CodeView {...stepProps("f1.1")} />);
    const region = screen.getByRole("region", { name: /src\/mempool\/mod.rs/ });
    expect(region).toHaveAttribute("data-kind", "context");
    expect(within(region).getByText(caption)).toBeInTheDocument();
    unmount();
    // The LLM calls it context, but the diff changed a line in it: no "unchanged" caption.
    const props = stepProps("f1.1");
    const { unmount: unmount2 } = render(<CodeView {...props} lines={{ ...props.lines, changed: [128] }} />);
    expect(screen.getByRole("region", { name: /src\/mempool\/mod.rs/ })).toHaveAttribute(
      "data-kind",
      "context",
    );
    expect(screen.queryByText(/Unchanged code/)).toBeNull();
    unmount2();
    render(<CodeView {...stepProps("f1.2")} />);
    expect(screen.getByRole("region", { name: /lines 88 to 96/ })).toHaveAttribute("data-kind", "modified");
    expect(screen.queryByText(/Unchanged code/)).toBeNull();
  });

  it("offers a keyboard-reachable comment action on every displayed line with one tab stop", async () => {
    const onLineAction = vi.fn();
    render(<CodeView {...stepProps("f1.2")} onLineAction={onLineAction} />);
    const buttons = screen.getAllByRole("button", { name: /^Comment on / });
    expect(buttons).toHaveLength(9 + 3);
    expect(buttons.filter((b) => b.tabIndex === 0)).toHaveLength(1);
    act(() => buttons[0]!.focus());
    await userEvent.keyboard("{ArrowDown}");
    expect(screen.getByRole("button", { name: "Comment on removed line 99" })).toHaveFocus();
    await userEvent.keyboard("{ArrowDown}{Enter}");
    expect(onLineAction).toHaveBeenLastCalledWith(
      { path: "src/rpc/client.rs", side: "head", start_line: null, line: 89 },
      { extend: false },
    );
    await userEvent.click(screen.getByRole("button", { name: "Comment on removed line 99" }));
    expect(onLineAction).toHaveBeenLastCalledWith(
      { path: "src/rpc/client.rs", side: "base", start_line: null, line: 99 },
      { extend: false },
    );
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
    const highlight: Highlight = async (_, fragments) =>
      fragments.map((lines) => lines.map((l) => [{ content: l, light: "#000000" }]));
    const thread = (seedFor("default", "tour") as Tour).threads[0]!;
    render(
      <CodeView
        lines={plain(1, [evil])}
        annotations={[note("x.a1", 1, 1, { type: "behaviour", was: evil, is: evil, text: null })]}
        threads={[{ ...thread, path: "src/a.rs", line: 1, author: evil, excerpt: evil }]}
        highlight={highlight}
      />,
    );
    await act(async () => {});
    expect(document.querySelector("[data-token]")).not.toBeNull();
    await userEvent.hover(document.querySelector("[data-highlight]")!);
    expect(document.querySelector("img")).toBeNull();
    expect(document.querySelector("a")).toBeNull();
    expect(document.querySelector("[data-highlight] [hidden]")).toHaveTextContent(evil);
  });

  describe("syntax highlighting", () => {
    it("paints plain text first, then the tokens of the displayed lines only", async () => {
      let resolve: (tokens: (Token[][] | null)[]) => void = () => {};
      const highlight = vi.fn<Highlight>(() => new Promise((r) => (resolve = r)));
      render(<CodeView lines={plain(7, ["let a = 1;", "let b = 2;"])} highlight={highlight} />);
      expect(highlight).toHaveBeenCalledWith(
        "src/a.rs",
        [["let a = 1;", "let b = 2;"]],
        expect.any(AbortSignal),
      );
      expect(document.querySelector("[data-line='7'] code")).toHaveTextContent("let a = 1;");
      expect(document.querySelector("[data-token]")).toBeNull();
      await act(async () =>
        resolve([
          [
            [{ content: "let", light: "#cf222e", dark: "#ff7b72" }, { content: " a = 1;" }],
            [{ content: "let b = 2;" }],
          ],
        ]),
      );
      const token = document.querySelector("[data-line='7'] [data-token]") as HTMLElement;
      expect(token).toHaveTextContent("let");
      expect(token.style.getPropertyValue("--tok-light")).toBe("#cf222e");
      expect(token.style.getPropertyValue("--tok-dark")).toBe("#ff7b72");
      expect(document.querySelector("[data-line='7'] code")).toHaveTextContent("let a = 1;");
    });

    it("highlights the range and its removed code in one call, range first", async () => {
      const highlight = vi.fn<Highlight>(async () => [
        [[{ content: "new();", light: "#111111" }]],
        null,
        [[{ content: "two();", light: "#222222" }]],
      ]);
      const blocks = [
        { after_line: 7, base_start: 3, lines: ["old();"], total_lines: 1 },
        { after_line: 7, base_start: 9, lines: ["two();"], total_lines: 1 },
      ];
      render(<CodeView lines={plain(7, ["new();"], { deleted: blocks })} highlight={highlight} />);
      await act(async () => {});
      expect(highlight).toHaveBeenCalledTimes(1);
      expect(highlight).toHaveBeenCalledWith(
        "src/a.rs",
        [["new();"], ["old();"], ["two();"]],
        expect.any(AbortSignal),
      );
      // A fragment the budget left out stays plain.
      expect(document.querySelector("[data-line='3'] [data-token]")).toBeNull();
      expect(document.querySelector("[data-line='9'] [data-token]")).not.toBeNull();
      expect(document.querySelector("[data-line='7'] [data-token]")).not.toBeNull();
    });

    it("keeps one budget for the whole view: many small removed blocks of pathological lines (CWE-770)", async () => {
      const real = createShikiHighlight({
        core: () => import("shiki/core"),
        engine: () => import("shiki/engine/javascript"),
        light: () => import("shiki/themes/github-light-default.mjs"),
        dark: () => import("shiki/themes/github-dark-default.mjs"),
        langs: { rust: () => import("shiki/langs/rust.mjs") } as ShikiModules["langs"],
      });
      await real("w.rs", [["fn a() {}"]]);
      const highlight = vi.fn(real);
      const line = `${"a+".repeat(999)}a`;
      const deleted = Array.from({ length: 40 }, (_, i) => ({
        after_line: 1,
        base_start: 1 + i * 5,
        lines: Array.from({ length: 5 }, () => line),
        total_lines: 5,
      }));
      render(<CodeView lines={plain(1, ["fn a() {}"], { deleted })} highlight={highlight} />);
      expect(highlight).toHaveBeenCalledTimes(1);
      const slice = await longestSlice(() => highlight.mock.results[0]!.value as Promise<unknown>);
      expect(slice).toBeLessThan(100);
      await act(async () => {});
      expect(document.querySelectorAll("[data-token]").length).toBeLessThanOrEqual(MAX_TOKENS);
      expect(document.querySelector("[data-line='196'] [data-token]")).toBeNull();
    }, 30_000);

    it("drops colours that no longer match the path or highlighter (same lines reference)", async () => {
      const lines = plain(1, ["let a = 1;"], { path: "src/a.rs" });
      const colour: Highlight = async (_, f) =>
        f.map((l) => l.map((t) => [{ content: t, light: "#111111" }]));
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
      ["tokens that do not match the text", async () => [[[{ content: "other" }]]]],
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

describe("CodeView: the reviewer's pending comments (owner: a comment must be findable again)", () => {
  const mine = (over: Partial<PendingComment> = {}): PendingComment => ({
    id: "c1",
    thread_id: "t1",
    path: "src/a.rs",
    kind: "line",
    side: "head",
    start_line: null,
    line: 2,
    body: "Is the retry bounded?",
    ...over,
  });
  const view = (comments: PendingComment[], extra: Partial<CodeViewProps> = {}) =>
    render(<CodeView lines={plain(1, ["a", "b", "c"])} comments={comments} {...extra} />);

  it("marks the line with a speech-bubble button named by the line, with a tooltip on what it does", () => {
    view([mine()]);
    const marker = screen.getByRole("button", { name: "Your pending comment at line 2" });
    expect(rowOf(marker)).toBe(2);
    expect(marker.querySelector("svg")).not.toBeNull();
    expect(marker).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByText("Is the retry bounded?")).toBeNull();
    expect(marker).toHaveAccessibleDescription(
      "Your comment in your GitHub pending review; nothing is published. Hover or click for its text.",
    );
    act(() => marker.focus());
    expect(marker).toHaveAccessibleDescription(
      "Is the retry bounded? Your comment in your GitHub pending review; nothing is published.",
    );
  });

  it("shows the comment's own text on hover, on a target of at least 24 px", async () => {
    const user = userEvent.setup();
    view([mine(), mine({ id: "c2", body: "And the backoff?" })]);
    const marker = screen.getByRole("button", { name: "Your 2 pending comments at line 2" });
    expect(marker.className).toMatch(/(^|\s)size-6(\s|$)/);
    await user.hover(marker);
    expect(screen.queryByRole("tooltip")).toBeNull();
    const tip = screen.getByRole("dialog", { name: "Your 2 pending comments at line 2" });
    expect(tip).toHaveTextContent("Is the retry bounded?");
    expect(tip).toHaveTextContent("And the backoff?");
    // A long comment is clamped; `block` would override the clamp's display.
    const body = within(tip).getByText("Is the retry bounded?");
    expect(body.className).toMatch(/(^|\s)line-clamp-6(\s|$)/);
    expect(body.className).not.toMatch(/(^|\s)block(\s|$)/);
  });

  it("opens the comment's text right at the line by click, and closes it again", async () => {
    const user = userEvent.setup();
    view([mine()]);
    const marker = screen.getByRole("button", { name: "Your pending comment at line 2" });
    await user.click(marker);
    // The click pins the popover; opening the text is its labelled action.
    expect(screen.queryByRole("list", { name: /^Your pending comments/ })).toBeNull();
    const open = screen.getByRole("button", { name: "Open the comment here" });
    expect(open).toHaveAttribute("aria-expanded", "false");
    await user.click(open);
    // The action closes the popup, which would cover the opened text.
    expect(screen.queryByRole("dialog")).toBeNull();
    const panel = screen.getByRole("list", { name: "Your pending comments at line 2" });
    await user.click(marker);
    expect(screen.getByRole("button", { name: "Hide the comment here" })).toHaveAttribute(
      "aria-expanded",
      "true",
    );
    expect(screen.getByRole("button", { name: "Hide the comment here" })).toHaveAttribute(
      "aria-controls",
      panel.id,
    );
    expect(within(panel).getByText("Is the retry bounded?")).toBeVisible();
    // Readable without edit mode; Edit and Delete only with their callbacks.
    expect(within(panel).queryByRole("textbox")).toBeNull();
    expect(within(panel).queryByRole("button")).toBeNull();
    expectTrustWording(panel.textContent ?? "");
    await user.click(screen.getByRole("button", { name: "Hide the comment here" }));
    expect(screen.queryByRole("list", { name: /^Your pending comments/ })).toBeNull();
  });

  it("an opened comment has Hide in its top-right corner; it closes the text and returns the focus to the marker", async () => {
    const user = userEvent.setup();
    view([mine()]);
    const marker = screen.getByRole("button", { name: "Your pending comment at line 2" });
    await user.click(marker);
    await user.click(screen.getByRole("button", { name: "Open the comment here" }));
    const hide = screen.getByRole("button", { name: "Hide your pending comment" });
    expect(hide).toHaveTextContent(/^Hide$/);
    expect(hide).toHaveAccessibleDescription(/marker/);
    hide.focus();
    await user.keyboard("{Enter}");
    expect(screen.queryByRole("list", { name: /^Your pending comments/ })).toBeNull();
    expect(marker).toHaveFocus();
  });

  it("opens by keyboard: Enter and Space", async () => {
    const user = userEvent.setup();
    view([mine()]);
    const marker = screen.getByRole("button", { name: "Your pending comment at line 2" });
    marker.focus();
    await user.keyboard("{Enter}");
    await user.tab();
    await user.tab();
    expect(screen.getByRole("button", { name: "Open the comment here" })).toHaveFocus();
    await user.keyboard("{Enter}");
    const panel = () => screen.queryByRole("list", { name: "Your pending comments at line 2" });
    expect(within(panel()!).getByText("Is the retry bounded?")).toBeVisible();
    // The action closed the popup and handed the focus back to the marker; Space pins it again.
    marker.focus();
    await user.keyboard(" ");
    await user.tab();
    await user.tab();
    await user.keyboard(" ");
    expect(panel()).toBeNull();
  });

  it("shows every comment of a line, with Edit and Delete acting on GitHub", async () => {
    const user = userEvent.setup();
    const onEditComment = vi.fn().mockResolvedValue(undefined);
    const onDeleteComment = vi.fn().mockResolvedValue(undefined);
    view([mine(), mine({ id: "c2", body: "And the backoff?" })], { onEditComment, onDeleteComment });
    await user.click(screen.getByRole("button", { name: "Your 2 pending comments at line 2" }));
    await user.click(screen.getByRole("button", { name: "Open the comments here" }));
    const panel = screen.getByRole("list", { name: "Your pending comments at line 2" });
    expect(
      within(panel)
        .getAllByRole("listitem")
        .map((li) => li.querySelector("p")?.textContent),
    ).toEqual(["Is the retry bounded?", "And the backoff?"]);
    await user.click(within(panel).getAllByRole("button", { name: /^Delete comment on/ })[1]!);
    expect(onDeleteComment).toHaveBeenCalledWith("c2");
    await user.click(within(panel).getAllByRole("button", { name: /^Edit comment on/ })[0]!);
    await user.type(within(panel).getByRole("textbox"), "!");
    await user.click(within(panel).getByRole("button", { name: "Save" }));
    expect(onEditComment).toHaveBeenCalledWith("c1", "Is the retry bounded?!");
  });

  it("names a removed line and a multi-line comment by its last line", () => {
    const { unmount } = render(
      <CodeView
        lines={plain(1, ["a", "b", "c"], { side: "base", changed: [3] })}
        comments={[mine({ side: "base", line: 3 })]}
      />,
    );
    expect(
      screen.getByRole("button", { name: "Your pending comment at removed line 3" }),
    ).toBeInTheDocument();
    unmount();
    view([mine({ start_line: 1, line: 3 })]);
    expect(rowOf(screen.getByRole("button", { name: "Your pending comment at lines 1–3" }))).toBe(3);
  });

  it("opens the comment asked for (from the pending-review list) without a click", () => {
    view([mine(), mine({ id: "c9", line: 3, body: "Other" })], { revealComment: "c9" });
    const panel = screen.getByRole("list", { name: "Your pending comments at line 3" });
    expect(within(panel).getByText("Other")).toBeVisible();
    expect(screen.queryByRole("list", { name: "Your pending comments at line 2" })).toBeNull();
  });

  it("puts a file comment on its linked line, or at the top of the file's code when that line is not shown", async () => {
    const user = userEvent.setup();
    const file = mine({ kind: "file", side: null, line: 2, body: "File-level note." });
    const { unmount } = view([file]);
    expect(rowOf(screen.getByRole("button", { name: "Your pending comment at line 2" }))).toBe(2);
    unmount();
    view([
      { ...file, line: 40 },
      { ...file, id: "f2", line: null, body: "No link." },
    ]);
    const top = screen.getByRole("button", { name: "Your 2 pending file comments on src/a.rs" });
    expect(top.closest("[data-line]")).toBeNull();
    await user.click(top);
    await user.click(screen.getByRole("button", { name: "Open the comments here" }));
    const panel = screen.getByRole("list", { name: "Your pending file comments on src/a.rs" });
    expect(within(panel).getByText("File-level note.")).toBeVisible();
    expect(within(panel).getByText("No link.")).toBeVisible();
    screen.getByRole("button", { name: "Hide your pending comments" }).focus();
    await user.keyboard("{Enter}");
    expect(screen.queryByRole("list", { name: "Your pending file comments on src/a.rs" })).toBeNull();
    expect(top).toHaveFocus();
  });
});

it("pairs replacement code and hides rule-covered changes only in the main view", () => {
  const lines: LinesResponse = {
    path: "a.rs",
    side: "head",
    from: 1,
    to: 3,
    total_lines: 3,
    lines: ["use std::fmt;", "new();", "work();"],
    changed: [1, 2, 3],
    context_only: [1],
    deleted: [
      {
        after_line: 1,
        base_start: 2,
        lines: ["old();"],
        total_lines: 1,
        replacement: { start_line: 2, end_line: 2 },
      },
    ],
    moved: [],
    comments: { scope: "not_in_diff" },
  };
  const { container, rerender } = render(<CodeView lines={lines} hideContextChanges />);
  expect(screen.getByText("Changed: 1 line replaced")).toBeVisible();
  expect(screen.queryByText(/Removed here/)).toBeNull();
  expect(screen.getByText("1 comment/import line hidden — in the classic diff")).toBeVisible();
  expect(container.querySelector('[data-line="1"]')).toBeNull();
  expect(
    container.querySelector('[data-line="2"][data-side="head"] [data-mark="changed"]'),
  ).toHaveTextContent("+");
  rerender(<CodeView lines={lines} hideContextChanges={false} />);
  expect(container.querySelector('[data-line="1"]')).not.toBeNull();
});

it("hides rule-covered removed comments as well as head lines", () => {
  const lines: LinesResponse = {
    path: "a.rs",
    side: "head",
    from: 1,
    to: 1,
    total_lines: 1,
    lines: ["work();"],
    changed: [],
    deleted: [{ after_line: 0, base_start: 1, lines: ["// old note"], total_lines: 1, context_only: [1] }],
    moved: [],
    comments: { scope: "not_in_diff" },
  };
  render(<CodeView lines={lines} />);
  expect(screen.queryByText("// old note")).toBeNull();
  expect(screen.getByText("1 comment/import line hidden — in the classic diff")).toBeVisible();
});

describe("CodeView: gutter markers (owner: explained, large enough, the icon's own tip first)", () => {
  const tour = seedFor("default", "tour") as Tour;
  const thread = { ...tour.threads[0]!, path: "src/a.rs", line: 2 };
  const own: PendingComment = {
    id: "c1",
    thread_id: "t1",
    path: "src/a.rs",
    kind: "line",
    side: "head",
    start_line: null,
    line: 2,
    body: "Is the retry bounded?",
  };

  it("an LLM remark marker is a 24 px button whose popover shows the full remark with an LLM tag and was and is as separate blocks", async () => {
    const user = userEvent.setup();
    render(
      <CodeView
        lines={plain(1, ["a", "b"])}
        annotations={[
          note("n1", 1, 2, { type: "behaviour", was: "retries once", is: "retries all", text: null }),
          note("n2", 1, 1, {
            type: "flag",
            text: "Potential double broadcast:",
            flag: {
              level: "red",
              scenario: ["The reply arrives late.", "The client sends again."],
              not_checked: "Not run.",
            },
          }),
        ]}
        band="medium"
      />,
    );
    const marker = screen.getByRole("button", { name: "LLM remark at line 1: Behaviour" });
    expect(marker.className).toMatch(/(^|\s)size-6(\s|$)/);
    await user.hover(document.querySelector("[data-highlight] [data-line='2']")!);
    expect(screen.queryByRole("tooltip")).toBeNull();
    await user.hover(marker);
    const popover = screen.getByRole("dialog", { name: "LLM remark at line 1: Behaviour" });
    expect(popover).toHaveTextContent("Behaviour · lines 1–2");
    const wasLabel = within(popover).getByText("Was:");
    const isLabel = within(popover).getByText("Is:");
    const was = wasLabel.parentElement!;
    const is = isLabel.parentElement!;
    expect(was).toHaveTextContent("Was: retries once");
    expect(is).toHaveTextContent("Is: retries all");
    expect(was).not.toBe(is);
    expect(was.parentElement).toBe(is.parentElement);
    expect(was.className).toMatch(/(^|\s)block(\s|$)/);
    expect(is.className).toMatch(/(^|\s)block(\s|$)/);
    expect(is.className).toMatch(/(^|\s)mt-/);
    expect(wasLabel).toHaveClass("font-semibold");
    expect(isLabel).toHaveClass("font-semibold");
    expect(popover).not.toHaveTextContent("Red flag");
    await user.unhover(marker);
    await user.hover(screen.getByRole("button", { name: "LLM remark at line 1: Red flag" }));
    const flag = screen.getByRole("dialog", { name: "LLM remark at line 1: Red flag" });
    expect(flag).toHaveTextContent("Red flag · line 1");
    expect(flag).toHaveTextContent("Potential double broadcast:");
    expect(
      within(flag)
        .getAllByRole("listitem")
        .map((i) => i.textContent),
    ).toEqual(["1. The reply arrives late.", "2. The client sends again."]);
    expect(within(flag).getByText("LLM")).toHaveAttribute("data-provenance", "llm");
    expect(within(flag).getByText("LLM")).toHaveAccessibleDescription(
      "This text was written by the LLM; it was not checked.",
    );
    expect(screen.queryByRole("tooltip")).toBeNull();
    expect(within(flag).queryByRole("button", { name: /Show in Inspector/ })).toBeNull();
    expectTrustWording(flag.textContent ?? "");
  });

  it("a pinned remark offers Show in Inspector for each remark; clicking it does not select the highlight", async () => {
    const user = userEvent.setup();
    const onShowInInspector = vi.fn();
    const onSelectAnnotation = vi.fn();
    render(
      <CodeView
        lines={plain(1, ["a"])}
        annotations={[note("n1", 1, 1)]}
        onShowInInspector={onShowInInspector}
        onSelectAnnotation={onSelectAnnotation}
      />,
    );
    await user.click(screen.getByRole("button", { name: "LLM remark at line 1: Note" }));
    const popover = screen.getByRole("dialog", { name: "LLM remark at line 1: Note" });
    await user.click(within(popover).getByRole("button", { name: /^Show in Inspector/ }));
    expect(onShowInInspector).toHaveBeenCalledWith({ kind: "annotation", id: "n1" });
    expect(onSelectAnnotation).not.toHaveBeenCalled();
  });

  it("a GitHub thread marker is a slate speech bubble whose popover shows author, excerpt and links", async () => {
    const user = userEvent.setup();
    const onShowInInspector = vi.fn();
    render(
      <CodeView lines={plain(1, ["a", "b"])} threads={[thread]} onShowInInspector={onShowInInspector} />,
    );
    const marker = screen.getByRole("button", { name: "GitHub thread by bob at line 2" });
    expect(marker.className).toMatch(/(^|\s)size-6(\s|$)/);
    expect(marker.className).toMatch(/(^|\s)text-thread(\s|$)/);
    expect(marker.querySelector("svg")).not.toBeNull();
    await user.hover(marker);
    const popover = screen.getByRole("dialog", { name: "GitHub thread at line 2" });
    expect(within(popover).getByText("GitHub")).toHaveAttribute("data-provenance", "github");
    expect(within(popover).getByText("GitHub")).toHaveAccessibleDescription(
      "Taken from GitHub as written there.",
    );
    expect(popover).toHaveTextContent("@bob");
    const excerpt = within(popover).getByText("Should we retry non-idempotent calls at all?");
    // `block` would override the clamp's display and show the whole text.
    expect(excerpt.className).toMatch(/(^|\s)line-clamp-4(\s|$)/);
    expect(excerpt.className).not.toMatch(/(^|\s)block(\s|$)/);
    expect(within(popover).queryByRole("link")).toBeNull();
    await user.click(marker);
    expect(within(popover).getByRole("link", { name: /Open on GitHub/ })).toHaveAttribute("href", thread.url);
    await user.click(within(popover).getByRole("button", { name: /^Show in Inspector/ }));
    expect(onShowInInspector).toHaveBeenCalledWith({ kind: "thread", id: thread.id });
  });

  it("omits Open on GitHub without a thread URL and Show in Inspector for a thread outside the steps", async () => {
    const user = userEvent.setup();
    render(
      <CodeView
        lines={plain(1, ["a", "b"])}
        threads={[{ ...thread, url: "", step_id: null }]}
        onShowInInspector={vi.fn()}
      />,
    );
    await user.click(screen.getByRole("button", { name: "GitHub thread by bob at line 2" }));
    const popover = screen.getByRole("dialog", { name: "GitHub thread at line 2" });
    expect(within(popover).queryByRole("link")).toBeNull();
    expect(within(popover).queryByRole("button", { name: /^Show in Inspector/ })).toBeNull();
    expect(within(popover).getByRole("button", { name: "Close" })).toBeVisible();
  });

  it("offers no Open on GitHub for a thread URL that is not a GitHub URL", async () => {
    const user = userEvent.setup();
    render(
      <CodeView lines={plain(1, ["a", "b"])} threads={[{ ...thread, url: "https://example.com/t/1" }]} />,
    );
    await user.click(screen.getByRole("button", { name: "GitHub thread by bob at line 2" }));
    const popover = screen.getByRole("dialog", { name: "GitHub thread at line 2" });
    expect(popover).not.toHaveTextContent("Open on GitHub");
  });

  it("names the step a thread of another step opens in, and says that it switches the step", async () => {
    const user = userEvent.setup();
    const onShowInInspector = vi.fn();
    render(
      <CodeView
        lines={plain(1, ["a", "b"])}
        threads={[thread]}
        onShowInInspector={onShowInInspector}
        otherThreadStep={() => "2.1"}
      />,
    );
    await user.click(screen.getByRole("button", { name: "GitHub thread by bob at line 2" }));
    const popover = screen.getByRole("dialog", { name: "GitHub thread at line 2" });
    expect(within(popover).queryByRole("button", { name: /^Show in Inspector/ })).toBeNull();
    const show = within(popover).getByRole("button", { name: "Show in step 2.1: GitHub thread by bob" });
    expect(show).toHaveTextContent("Show in step 2.1");
    expect(show).toHaveAccessibleDescription(/switches to step 2\.1/i);
    await user.click(show);
    expect(onShowInInspector).toHaveBeenCalledWith({ kind: "thread", id: thread.id });
  });

  it("≠, − and + are read with their line and are no tooltip triggers (the ? legend explains them)", () => {
    const props = stepProps("f5.1");
    const { unmount } = render(<CodeView {...props} />);
    const differs = document.querySelector("[data-mark=differs]")!;
    expect(differs).not.toHaveAttribute("title");
    expect(differs.closest("button")).toBeNull();
    expect(differs).toHaveTextContent(/differs from the moved original/i);
    expect([differs, differs.parentElement!].some((e) => e.hasAttribute("aria-describedby"))).toBe(false);
    unmount();
    render(
      <CodeView
        lines={plain(1, ["a"], {
          deleted: [{ after_line: 1, base_start: 5, lines: ["old"], total_lines: 1 }],
        })}
      />,
    );
    const removed = document.querySelector("[data-mark=removed]")!;
    expect(removed).not.toHaveAttribute("title");
    expect(removed).toHaveTextContent(/removed line/i);
    expect([removed, removed.parentElement!].some((e) => e.hasAttribute("aria-describedby"))).toBe(false);
  });

  it("makes room in the gutter for every marker of a line", () => {
    render(
      <CodeView
        lines={plain(1, ["a", "b"])}
        annotations={[note("n1", 2, 2)]}
        threads={[thread]}
        comments={[own]}
      />,
    );
    const region = screen.getByRole("region");
    expect(parseInt(region.style.getPropertyValue("--gutter"))).toBeGreaterThanOrEqual(12 + 3 * 24);
  });

  it("keeps lines with a marker visible when comment and import changes are hidden", () => {
    const lines = plain(1, ["// a", "// b", "use c;", "// d", "e();"], {
      changed: [1, 2, 3, 4, 5],
      context_only: [1, 2, 3, 4],
      deleted: [
        { after_line: 5, base_start: 9, lines: ["// old"], total_lines: 1, context_only: [9] },
        { after_line: 5, base_start: 20, lines: ["// gone"], total_lines: 1, context_only: [20] },
      ],
    });
    const { container } = render(
      <CodeView
        lines={lines}
        hideContextChanges
        annotations={[note("n1", 3, 3)]}
        threads={[{ ...thread, line: 2 }]}
        comments={[
          { ...own, line: 1 },
          { ...own, id: "c2", side: "base", line: 9 },
        ]}
      />,
    );
    expect(lineNumbers(container, "[data-line][data-side=head]")).toEqual([1, 2, 3, 5]);
    expect(lineNumbers(container, "[data-line][data-side=base]")).toEqual([9]);
    expect(screen.getByText("2 comment/import lines hidden — in the classic diff")).toBeVisible();
  });

  it("flashes the line an evidence link jumps to, even a hidden comment line", async () => {
    const { container } = render(
      <CodeView
        lines={plain(1, ["// a", "b"], { changed: [1], context_only: [1] })}
        hideContextChanges
        flashLine={1}
      />,
    );
    const row = container.querySelector('[data-line="1"][data-side="head"]');
    expect(row).not.toBeNull();
    await waitFor(() => expect(row).toHaveAttribute("data-flash"));
  });
});
