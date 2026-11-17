// Two analyses of real PRs from the QA pass on the real stack (2026-11-17): dashpay/dash-evo-tool#1169 (1 flow,
// 3 steps) and dashpay/tenderdash#1578 (1 flow, 6 steps). The files' text is synthetic: long lines, the
// tours' changed lines marked, so layout and reading can be checked at realistic sizes.
import type { AskQuestion, LineRange, LineSide, LinesResponse, Tour } from "../../api/generated/types";
import det1169 from "./det1169-tour.json";
import td1578 from "./td1578-tour.json";
import td1578Questions from "./td1578-questions.json";

/** The real tours by scenario name. */
export const REAL_TOURS = {
  "real-small": { ...td1578, analysis: { ...td1578.analysis, usage: null } } as Tour,
  "real-tiny": { ...det1169, analysis: { ...det1169.analysis, usage: null } } as Tour,
} as const;

/** Questions asked on #1578 (one answered, with structured evidence). */
export const REAL_QUESTIONS = td1578Questions as AskQuestion[];

/** Changed line ranges of `path` on `side` named by the tour (steps and Not analysed rows). */
function changedRanges(tour: Tour, path: string, side: LineSide): LineRange[] {
  const fromSteps = tour.flows.flatMap((f) =>
    f.steps
      .filter((s) => s.facts.file === path && s.facts.side === side)
      .flatMap((s) => s.facts.changed_lines),
  );
  const fromRows = tour.quiet.not_analysed.flatMap((section) =>
    section.items.filter((i) => i.file === path && i.side === side).flatMap((i) => i.ranges ?? []),
  );
  return [...fromSteps, ...fromRows];
}

/** Synthetic text of line `n`: indented code, every fifth line long (about 170 characters). */
function text(path: string, n: number): string {
  const indent = "\t".repeat(1 + (n % 3));
  const long = n % 5 === 0 ? ` // ${"a long expression that does not fit a narrow column ".repeat(3)}` : "";
  return `${indent}value${n} := compute(${JSON.stringify(path.split("/").at(-1))}, ${n})${long}`;
}

/** Lines `from..=to` of a file of `tour` (any path: the text is synthetic). */
export function realLines(tour: Tour, path: string, side: LineSide, from: number, to: number): LinesResponse {
  const ranges = changedRanges(tour, path, side);
  const ends = [
    ...ranges.map((r) => r.end_line),
    ...tour.flows.flatMap((f) =>
      f.steps.filter((s) => s.facts.file === path).map((s) => s.facts.range.end_line),
    ),
  ];
  const total = Math.max(400, ...ends.map((e) => e + 60));
  const end = Math.min(to, total);
  const changed: number[] = [];
  for (const r of ranges) for (let n = r.start_line; n <= r.end_line; n += 1) changed.push(n);
  return {
    path,
    side,
    from,
    to: end,
    total_lines: total,
    lines: Array.from({ length: Math.max(0, end - from + 1) }, (_, i) => text(path, from + i)),
    changed: [...new Set(changed)].filter((n) => n >= from && n <= end).sort((a, b) => a - b),
    deleted: [],
    moved: [],
    comments: {
      scope: "in_diff",
      line_thread_ranges: ranges.map((r) => ({
        start_line: Math.max(1, r.start_line - 3),
        end_line: r.end_line + 3,
      })),
    },
  };
}
