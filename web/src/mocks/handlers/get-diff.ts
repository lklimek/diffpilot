import { HttpResponse } from "msw";
import type { DiffLine, DiffResponse, LineSide } from "../../api/generated/types";
import { mockLines } from "../lines";
import { apiError, forAnalysis, route } from "./common";

/**
 * The classic diff of `from..=to` on `side`, derived from the whole-file lines seed: changed lines are added
 * (removed on the base side), deletion blocks are removed lines at their marker, the rest is context.
 */
export function diffFor(path: string, side: LineSide, from: number, to: number): DiffResponse | undefined {
  const file = mockLines(path, side, 1, Number.MAX_SAFE_INTEGER);
  if (!file) return undefined;
  const changed = new Set(file.changed);
  const lines: DiffLine[] = [];
  const inRange = (line: number) => line >= from && line <= to;
  let old = 0;
  // One past the last line: a deletion at the end of the file.
  for (let line = 1; line <= file.total_lines + 1; line += 1) {
    const text = file.lines[line - 1] ?? "";
    if (side === "base") {
      if (inRange(line)) {
        lines.push({
          kind: changed.has(line) ? "removed" : "context",
          old_line: line,
          new_line: null,
          text,
          eol: null,
        });
      }
      continue;
    }
    for (const block of file.deleted.filter((d) => d.after_line === line - 1)) {
      block.lines.forEach((removed, i) => {
        if (inRange(line) || inRange(line - 1)) {
          lines.push({
            kind: "removed",
            old_line: block.base_start + i,
            new_line: null,
            text: removed,
            eol: null,
          });
        }
      });
      old = block.base_start + block.total_lines - 1;
    }
    if (line > file.total_lines) break;
    if (changed.has(line)) {
      if (inRange(line)) lines.push({ kind: "added", old_line: null, new_line: line, text, eol: null });
    } else {
      old += 1;
      if (inRange(line)) lines.push({ kind: "context", old_line: old, new_line: line, text, eol: null });
    }
  }
  if (lines.every((l) => l.kind === "context")) return { path, side, hunks: [] };
  const olds = lines.filter((l) => l.kind !== "added").map((l) => l.old_line ?? 0);
  const news = lines.filter((l) => l.kind !== "removed").map((l) => l.new_line ?? 0);
  const header = `@@ -${olds[0] ?? 0},${olds.length} +${news[0] ?? from},${news.length} @@`;
  return { path, side, hunks: [{ header, lines }] };
}

/** `GET /api/v1/analyses/{id}/diff`. */
export const getDiff = route(
  "getDiff",
  forAnalysis(({ request }) => {
    const query = new URL(request.url).searchParams;
    const path = query.get("path");
    const side = query.get("side");
    const from = Number(query.get("from"));
    const to = Number(query.get("to"));
    if (!path || (side !== "head" && side !== "base") || !Number.isInteger(from) || !Number.isInteger(to)) {
      return apiError(400, "invalid_request", "The line range is malformed.");
    }
    if (from < 1 || to < from) return apiError(400, "invalid_request", "The line range is malformed.");
    const diff = diffFor(path, side, from, to);
    if (!diff) return apiError(404, "not_found", "This file is not part of the analysis.");
    return HttpResponse.json(diff);
  }),
);
