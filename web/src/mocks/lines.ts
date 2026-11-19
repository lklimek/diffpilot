// The mocks' source of file lines: a real scenario's synthetic files, else the fixture PR's seed.
import type { LineSide, LinesResponse, Tour } from "../api/generated/types";
import { fixtureState, linesFor, seedFor } from "./fixtures";
import { realLines } from "./real";
import { realTour, scenario } from "./scenarios";

const testSources = import.meta.glob<string>("../../../fixtures/pr-482/repo/{base,head}/tests/**/*.txt", {
  eager: true,
  query: "?raw",
  import: "default",
});

function testLines(path: string, side: LineSide, from: number, to: number): LinesResponse | undefined {
  const text = testSources[`../../../fixtures/pr-482/repo/${side}/${path}.txt`];
  if (text === undefined) return undefined;
  const lines = text.replace(/\n$/, "").split("\n");
  const items = (seedFor("default", "tour") as Tour).quiet.not_analysed
    .flatMap((s) => s.items)
    .filter((i) => i.file === path && i.side === side);
  const changed = lines
    .map((_, i) => i + 1)
    .filter(
      (n) =>
        n >= from &&
        n <= to &&
        items.some((i) => i.ranges === null || i.ranges.some((r) => n >= r.start_line && n <= r.end_line)),
    );
  return {
    path,
    side,
    from,
    to: Math.min(to, lines.length),
    total_lines: lines.length,
    lines: lines.slice(from - 1, to),
    changed,
    deleted: [],
    moved: [],
    comments: { scope: "in_diff", line_thread_ranges: [] },
  };
}

/** Lines `from..=to` of `path` on `side`; undefined when the file is not part of the analysis. */
export function mockLines(path: string, side: LineSide, from: number, to: number): LinesResponse | undefined {
  const tour = realTour();
  const answer = tour
    ? realLines(tour, path, side, from, to)
    : (linesFor(fixtureState(), path, side, from, to) ??
      (scenario().showLess ? testLines(path, side, from, to) : undefined));
  if (answer && scenario().showLess && path === "src/rpc/client.rs" && side === "head") {
    answer.context_only = answer.changed.filter((line) => line === 89);
    answer.deleted = answer.deleted.map((block) => ({
      ...block,
      ...(block.base_start === 99
        ? { context_only: [99], replacement: { start_line: 89, end_line: 89 } }
        : {}),
      ...(block.base_start === 101 ? { replacement: { start_line: 91, end_line: 96 } } : {}),
    }));
  }
  if (answer && scenario().showLess && path === "src/config.rs" && side === "head") {
    answer.deleted = answer.deleted.map((block) => ({
      ...block,
      replacement: { start_line: 23, end_line: 23 },
    }));
  }
  return answer && scenario().lines ? scenario().lines!(answer) : answer;
}
