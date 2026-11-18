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

  it("keeps the changed sign and the remark marker apart, each with its own tip", async () => {
    render(<CodeView lines={LINES} annotations={[REMARK]} band="low" hideContextChanges={false} />);
    const line = row("head", 363)!;
    const sign = line.querySelector<HTMLElement>("[data-mark=changed]")!;
    const marker = line.querySelector<HTMLElement>("[data-mark=remark]")!;
    expect(sign.parentElement?.closest("span.flex")?.className).toMatch(/(^|\s)gap-1(\s|$)/);
    expect(marker).toHaveAccessibleName("LLM remark at line 363");
    await userEvent.hover(sign);
    expect(screen.getByRole("tooltip")).toHaveTextContent("Added line (from the diff).");
  });
});

describe("CodeView: hidden comment and import lines fold in place", () => {
  it("shows a fold row where lines are hidden, inside the remark's bar; Show and Hide toggle that run only", async () => {
    render(<CodeView lines={LINES} annotations={[REMARK]} band="low" hideContextChanges />);
    for (const line of [366, 367, 368]) expect(row("head", line)).toBeNull();
    const fold = screen.getByText("3 comment lines hidden").closest<HTMLElement>("[data-fold]")!;
    expect(before(row("head", 365)!, fold)).toBe(true);
    expect(before(fold, row("head", 369)!)).toBe(true);
    expect(document.querySelector("[data-highlight]")).toContainElement(fold);
    const show = within(fold).getByRole("button", { name: "Show 3 hidden comment lines" });
    expect(show.className).toMatch(/min-h-6/);
    expect(show).toHaveAccessibleDescription(/classic diff/);
    // The removed comment lines of the second block fold the same way; the total stays.
    expect(screen.getByText("2 removed comment lines hidden")).toBeVisible();
    expect(screen.getByText("5 comment/import lines hidden — in the classic diff")).toBeVisible();

    await userEvent.click(show);
    for (const line of [366, 367, 368]) {
      expect(row("head", line)!.querySelector("[data-mark=changed]")).toHaveTextContent("+");
    }
    expect(row("base", 368)).toBeNull();
    const shown = screen.getByText("3 comment lines shown").closest<HTMLElement>("[data-fold]")!;
    await userEvent.click(within(shown).getByRole("button", { name: "Hide 3 comment lines" }));
    expect(row("head", 366)).toBeNull();
  });

  it("is reachable by keyboard", async () => {
    render(<CodeView lines={LINES} annotations={[REMARK]} band="low" hideContextChanges />);
    const show = screen.getByRole("button", { name: "Show 3 hidden comment lines" });
    act(() => show.focus());
    await userEvent.keyboard("{Enter}");
    expect(row("head", 366)).not.toBeNull();
  });

  it.each([
    [["use std::fmt;", "use std::io;"], "2 import lines hidden"],
    [["use std::fmt;", "// a note"], "2 comment and import lines hidden"],
  ])("names what is hidden: %j", (texts, text) => {
    const lines: LinesResponse = {
      ...LINES,
      from: 1,
      to: 3,
      lines: [...texts, "work();"],
      changed: [1, 2],
      context_only: [1, 2],
      deleted: [],
    };
    render(<CodeView lines={lines} hideContextChanges />);
    expect(screen.getByText(text)).toBeVisible();
  });
});
