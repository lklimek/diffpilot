import type {
  Annotation,
  GithubThread,
  LineSide,
  LinesResponse,
  PendingComment,
} from "../../api/generated/types";
import { commentsByLine, lineKey } from "../comment/LineComments";

/** What puts a marker on a line: the reviewer's comments, GitHub threads, LLM remarks (on `lines.side`). */
export interface LineMarkers {
  annotations?: readonly Annotation[];
  threads?: readonly GithubThread[];
  comments?: readonly PendingComment[];
}

/** Line numbers "hide comment and import changes" hides from view. */
export interface HiddenLines {
  /** Lines of `lines.side`. */
  rows: Set<number>;
  /** Base lines of the removed blocks. */
  removed: Set<number>;
}

/**
 * Rule-covered (`context_only`) lines hidden by "hide comment and import changes": none when `hide` is off, and
 * never a line carrying a marker (own comment, GitHub thread, inside an LLM remark), so no marker disappears.
 */
export function hiddenLines(lines: LinesResponse, hide: boolean, markers: LineMarkers): HiddenLines {
  if (!hide) return { rows: new Set(), removed: new Set() };
  const marked = new Set(commentsByLine(markers.comments ?? [], lines.path).keys());
  for (const t of markers.threads ?? []) {
    if (t.path === lines.path && t.side !== null && t.line !== null) marked.add(lineKey(t.side, t.line));
  }
  for (const a of markers.annotations ?? []) {
    const last = Math.min(a.range.end_line, lines.to);
    for (let line = Math.max(a.range.start_line, lines.from); line <= last; line++) {
      marked.add(lineKey(lines.side, line));
    }
  }
  const unmarked = (side: LineSide, list: readonly number[] = []) =>
    list.filter((line) => !marked.has(lineKey(side, line)));
  return {
    rows: new Set(unmarked(lines.side, lines.context_only)),
    removed: new Set(lines.deleted.flatMap((block) => unmarked("base", block.context_only))),
  };
}
