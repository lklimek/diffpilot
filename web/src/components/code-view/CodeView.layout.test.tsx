// A remark range crossing a replaced line and hidden comment lines (owner screenshot of the Tour, 2026-11-18).
import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { Annotation, LinesResponse } from "../../api/generated/types";
import { CodeView } from "./CodeView";

const LINES: LinesResponse = {
  path: "src/wallet.rs",
  side: "head",
  from: 361,
  to: 370,
  total_lines: 400,
  lines: [
    "fn check(&self) -> Result<()> {",
    "    let result = self",
    "        .core_wallet",
    "        .check_core_transaction(tx)",
    "        .await;",
    "    // retry once on a timeout",
    "    // then give up",
    "    // and log the failure",
    "    result?",
    "}",
  ],
  changed: [363, 366, 367, 368, 369],
  context_only: [366, 367, 368],
  deleted: [
    {
      after_line: 362,
      base_start: 362,
      lines: ["        .wallet"],
      total_lines: 1,
      replacement: { start_line: 363, end_line: 363 },
    },
    {
      after_line: 369,
      base_start: 368,
      lines: ["    // old note", "    // older note"],
      total_lines: 2,
      context_only: [368, 369],
    },
  ],
  moved: [],
  comments: { scope: "in_diff", line_thread_ranges: [] },
};

const REMARK: Annotation = {
  id: "s.a1",
  type: "behaviour",
  basis: "inference",
  range: { start_line: 363, end_line: 369 },
  was: "checks the wallet",
  is: "checks the core transaction",
  text: null,
  evidence: [],
};

const row = (side: "head" | "base", line: number) =>
  document.querySelector<HTMLElement>(`[data-line="${line}"][data-side="${side}"]`);
const before = (a: Node, b: Node) => Boolean(a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING);

describe("CodeView: one remark over a replaced line and hidden comment lines", () => {
  it("draws one remark as one label, one marker and one bar, across the replaced block", () => {
    render(<CodeView lines={LINES} annotations={[REMARK]} band="low" hideContextChanges={false} />);
    const highlights = document.querySelectorAll("[data-highlight]");
    expect(highlights).toHaveLength(1);
    expect(screen.getAllByText("● LOW · LLM remark: behaviour")).toHaveLength(1);
    expect(document.querySelectorAll("[data-mark=remark]")).toHaveLength(1);
    const run = highlights[0]!;
    for (const line of [363, 364, 365, 369]) expect(run).toContainElement(row("head", line));
    // The label sits at the top of the range, above the replaced block: removed 362 and its replacement 363
    // stay next to each other.
    const label = screen.getByText("● LOW · LLM remark: behaviour");
    const changed = screen.getByText("Changed: 1 line replaced");
    expect(before(label, changed)).toBe(true);
    const removed = row("base", 362)!;
    expect(removed.nextElementSibling).toBe(row("head", 363));
  });

  it("marks the range with a bar only: unchanged lines inside look like unchanged lines outside", () => {
    render(<CodeView lines={LINES} annotations={[REMARK]} band="low" hideContextChanges={false} />);
    const inside = row("head", 364)!;
    const outside = row("head", 361)!;
    expect(inside).not.toHaveAttribute("data-changed");
    expect(inside.className).not.toMatch(/(^|\s)bg-/);
    expect(inside.className).toBe(outside.className);
    const run = document.querySelector("[data-highlight]")!;
    expect(run.className).not.toMatch(/bg-(high|med|low|llm)-bg|outline/);
    expect(run).toHaveAttribute("data-bar", "dotted");
    // Changed lines keep their change look: the sign and, for removed code, the removed fill.
    expect(row("head", 363)!.querySelector("[data-mark=changed]")).toHaveTextContent("+");
    expect(row("base", 362)!.className).toMatch(/bg-rm-bg/);
  });

  it("keeps the changed sign and the remark marker apart; the sign is read with its line, not a tooltip", () => {
    render(<CodeView lines={LINES} annotations={[REMARK]} band="low" hideContextChanges={false} />);
    const line = row("head", 363)!;
    const sign = line.querySelector<HTMLElement>("[data-mark=changed]")!;
    const marker = line.querySelector<HTMLElement>("[data-mark=remark]")!;
    expect(sign.parentElement?.closest("span.flex")?.className).toMatch(/(^|\s)gap-1(\s|$)/);
    expect(marker).toHaveAccessibleName(/^LLM remark at line 363: /);
    expect(sign).toHaveTextContent("+added line");
    expect([sign, sign.parentElement!].some((e) => e.hasAttribute("aria-describedby"))).toBe(false);
  });
});

describe("CodeView: hidden comment and import lines fold in place", () => {
  it("shows a fold row where lines are hidden, inside the remark's bar; Show and Hide toggle that run only", async () => {
    render(<CodeView lines={LINES} annotations={[REMARK]} band="low" hideContextChanges />);
    for (const line of [366, 367, 368]) expect(row("head", line)).toBeNull();
    const fold = screen.getByText("3 comment/import lines hidden").closest<HTMLElement>("[data-fold]")!;
    expect(before(row("head", 365)!, fold)).toBe(true);
    expect(before(fold, row("head", 369)!)).toBe(true);
    expect(document.querySelector("[data-highlight]")).toContainElement(fold);
    const show = within(fold).getByRole("button", { name: "Show 3 hidden comment/import lines" });
    expect(show.className).toMatch(/min-h-6/);
    expect(show).toHaveAccessibleDescription(/classic diff/);
    // The removed comment lines of the second block fold the same way; the total stays.
    expect(screen.getByText("2 removed comment/import lines hidden")).toBeVisible();
    expect(screen.getByText("5 comment/import lines hidden — in the classic diff")).toBeVisible();

    await userEvent.click(show);
    for (const line of [366, 367, 368]) {
      expect(row("head", line)!.querySelector("[data-mark=changed]")).toHaveTextContent("+");
    }
    expect(row("base", 368)).toBeNull();
    const shown = screen.getByText("3 comment/import lines shown").closest<HTMLElement>("[data-fold]")!;
    await userEvent.click(within(shown).getByRole("button", { name: "Hide 3 comment/import lines" }));
    expect(row("head", 366)).toBeNull();
  });

  it("is reachable by keyboard", async () => {
    render(<CodeView lines={LINES} annotations={[REMARK]} band="low" hideContextChanges />);
    const show = screen.getByRole("button", { name: "Show 3 hidden comment/import lines" });
    act(() => show.focus());
    await userEvent.keyboard("{Enter}");
    expect(row("head", 366)).not.toBeNull();
  });

  it("names hidden lines neutrally: the diff does not say which are comments and which imports", () => {
    const lines: LinesResponse = {
      ...LINES,
      from: 1,
      to: 3,
      lines: ["mod tests;", "use the cache only once", "work();"],
      changed: [1, 2],
      context_only: [1, 2],
      deleted: [],
    };
    render(<CodeView lines={lines} hideContextChanges />);
    expect(screen.getByText("2 comment/import lines hidden")).toBeVisible();
  });

  it("closes runs opened with Show when the top-bar switch is turned off and on", async () => {
    const { rerender } = render(
      <CodeView lines={LINES} annotations={[REMARK]} band="low" hideContextChanges />,
    );
    await userEvent.click(screen.getByRole("button", { name: "Show 3 hidden comment/import lines" }));
    expect(row("head", 366)).not.toBeNull();
    rerender(<CodeView lines={LINES} annotations={[REMARK]} band="low" hideContextChanges={false} />);
    rerender(<CodeView lines={LINES} annotations={[REMARK]} band="low" hideContextChanges />);
    expect(row("head", 366)).toBeNull();
    expect(screen.getByRole("button", { name: "Show 3 hidden comment/import lines" })).toBeVisible();
  });
});

describe("CodeView: a remark starting inside a replacement", () => {
  // Line 362 is replaced by 363-365; the remark starts on the second replacing line, 364. Line 363 is a changed
  // comment line that folds.
  const lines: LinesResponse = {
    ...LINES,
    changed: [363, 364, 365],
    context_only: [363],
    deleted: [{ ...LINES.deleted[0]!, replacement: { start_line: 363, end_line: 365 } }],
  };
  const remark: Annotation = { ...REMARK, range: { start_line: 364, end_line: 366 } };

  it("puts the marker on the remark's first line, which never folds", () => {
    render(<CodeView lines={lines} annotations={[remark]} band="low" hideContextChanges />);
    const markers = document.querySelectorAll("[data-mark=remark]");
    expect(markers).toHaveLength(1);
    expect(markers[0]).toHaveAccessibleName(/^LLM remark at line 364: /);
    expect(row("head", 364)).toContainElement(markers[0] as HTMLElement);
    expect(row("head", 363)).toBeNull();
  });
});
