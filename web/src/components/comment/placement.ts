import type { CommentTarget, LinesResponse } from "../../api/generated/types";

/**
 * Where GitHub takes a comment on a displayed line (ADR-006): line thread, file comment, nowhere, or
 * `on_post` when the displayed lines do not tell (the server decides when the comment is posted).
 */
export type CommentPlacement = "line" | "file" | "not_in_diff" | "on_post";

/** Preview of the placement of a comment on `target`, from the deterministic facts of the displayed `lines`. */
export function commentPlacement(lines: LinesResponse, target: CommentTarget): CommentPlacement {
  const scope = lines.comments;
  if (scope.scope === "not_in_diff") return "not_in_diff";
  const first = target.start_line ?? target.line;
  if (target.side !== lines.side) {
    // Removed lines shown at deletion markers: one deletion block lies inside one hunk; for a range across
    // blocks the response does not carry the hunk boundaries of that side, so the server decides.
    const inOneBlock = lines.deleted.some(
      (d) => d.base_start <= first && target.line <= d.base_start + d.total_lines - 1,
    );
    return inOneBlock ? "line" : "on_post";
  }
  const inOneHunk = scope.line_thread_ranges.some((r) => r.start_line <= first && target.line <= r.end_line);
  return inOneHunk ? "line" : "file";
}
