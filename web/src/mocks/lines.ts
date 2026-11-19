// The mocks' source of file lines: a real scenario's synthetic files, else the fixture PR's seed.
import type { FilesResponse, LineSide, LinesResponse, Tour } from "../api/generated/types";
import filesSeed from "./files-482.json";
import { fixtureState, linesFor, seedFor } from "./fixtures";
import { realLines } from "./real";
import { realTour, scenario } from "./scenarios";

const repoSources = import.meta.glob<string>("../../../fixtures/pr-482/repo/{base,head}/**/*", {
  eager: true,
  query: "?raw",
  import: "default",
});

// A file of the fixture repository the lines seed does not hold (tests, docs): changed lines are its Not analysed
// ranges.
function repoLines(path: string, side: LineSide, from: number, to: number): LinesResponse | undefined {
  const at = `../../../fixtures/pr-482/repo/${side}/${path}`;
  const text = repoSources[`${at}.txt`] ?? repoSources[at];
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

// The server's `replacement` (http-api.md "Deterministic context and replacement lines"): the head range of the
// same diff hunk; the lines seed predates the field.
function withReplacements(answer: LinesResponse): LinesResponse {
  const file = (filesSeed as FilesResponse).files.find((f) => f.path === answer.path);
  if (!file || answer.side !== "head") return answer;
  return {
    ...answer,
    deleted: answer.deleted.map((block) => {
      const hunk = file.hunks.find(
        (h) =>
          h.base && h.head && h.base.start_line <= block.base_start && block.base_start <= h.base.end_line,
      );
      return hunk?.head && !block.replacement ? { ...block, replacement: hunk.head } : block;
    }),
  };
}

/** Lines `from..=to` of `path` on `side`; undefined when the file is not part of the analysis. */
export function mockLines(path: string, side: LineSide, from: number, to: number): LinesResponse | undefined {
  const tour = realTour();
  const seeded = linesFor(fixtureState(), path, side, from, to);
  const answer = tour
    ? realLines(tour, path, side, from, to)
    : seeded
      ? withReplacements(seeded)
      : repoLines(path, side, from, to);
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
