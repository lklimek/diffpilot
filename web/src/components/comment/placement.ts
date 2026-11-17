import type { CommentTarget, LinesResponse } from "../../api/generated/types";

/** Where GitHub takes a comment on a displayed line (ADR-006): line thread, file comment, or nowhere. */
export type CommentPlacement = "line" | "file" | "not_in_diff";

/** Placement of a comment on `target`, from the deterministic comment scope of the displayed `lines`. */
export function commentPlacement(lines: LinesResponse, target: CommentTarget): CommentPlacement {
  const scope = lines.comments;
  if (scope.scope === "not_in_diff") return "not_in_diff";
  // A removed line shown at a deletion marker of a head range is inside a hunk by definition.
  if (target.side !== lines.side) return "line";
  const first = target.start_line ?? target.line;
  const inOneHunk = scope.line_thread_ranges.some((r) => r.start_line <= first && target.line <= r.end_line);
  return inOneHunk ? "line" : "file";
}
