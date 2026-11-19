import type { Counters, LineRange, NotAnalysedCategory } from "../../api/generated/types";

/** Menu and heading name of a Not analysed section. */
export const NOT_ANALYSED_LABEL: Record<NotAnalysedCategory, string> = {
  tests: "Tests",
  docs_comments: "Docs & comments",
  generated_mechanical: "Mechanical & generated",
};

/** Number of lines in the ranges. */
export const lineCount = (ranges: readonly LineRange[]): number =>
  ranges.reduce((n, r) => n + r.end_line - r.start_line + 1, 0);

/** Number of distinct paths (a file can have a base and a head entry). */
export const fileCount = (items: readonly { file: string }[]): number =>
  new Set(items.map((i) => i.file)).size;

/** `n thing` or `n things`. */
export const plural = (n: number, one: string, many = `${one}s`): string => `${n} ${n === 1 ? one : many}`;

/** Raw test diff deletions include lines replaced in the same hunk. */
export const testLines = (counters: Counters): string =>
  `${plural(counters.lines_removed_from_tests, "test line")} changed or removed, ${counters.lines_added_to_tests === undefined ? "additions not checked" : `${counters.lines_added_to_tests} added`}`;
