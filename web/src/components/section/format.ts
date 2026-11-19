import type {
  Annotation,
  AnnotationType,
  Counters,
  Evidence,
  LineRange,
  ThreadsStatus,
} from "../../api/generated/types";
import { FLAG_LOOK, flagLevel } from "../flag/flags";

/** `file:line` of an evidence reference; base-side lines are marked. */
export const evidenceText = (e: Evidence) => `${e.file}:${e.line}${e.side === "base" ? " (base)" : ""}`;

/** Reader-facing name of each annotation type. */
export const ANNOTATION: Record<AnnotationType, string> = {
  behaviour: "Behaviour",
  consequence: "Consequence",
  flag: "Flag",
  note: "Note",
};

/** Reader-facing name of an annotation: a flag by its level ("Red flag"), others by type. */
export const annotationName = (a: Annotation) => {
  const level = flagLevel(a);
  return level ? FLAG_LOOK[level].name : ANNOTATION[a.type];
};

/** `a–b`, or `a` for a one-line range. */
export const rangeText = (r: LineRange) =>
  r.start_line === r.end_line ? `${r.start_line}` : `${r.start_line}–${r.end_line}`;

/**
 * The dependency counter as fact lines: the changed dependencies, and the dependency files that were not
 * checked. A count of zero is left out when some files were not checked (it would read as "none changed").
 */
export function dependencyFacts(counters: Counters): string[] {
  const changed = counters.dependencies_changed;
  const unchecked = counters.dependency_files_not_checked;
  const lines: string[] = [];
  if (changed.length > 0 || unchecked.length === 0) {
    const count = `${changed.length} ${changed.length === 1 ? "dependency" : "dependencies"} changed`;
    lines.push(changed.length > 0 ? `${count}: ${changed.join(", ")}` : count);
  }
  if (unchecked.length > 0) lines.push(`Dependency files not checked: ${unchecked.join(", ")}`);
  return lines;
}

/** Why a list of review threads is not complete (`partial`, `not_checked`); null when GitHub gave all of them. */
export function threadsNote(status: ThreadsStatus): string | null {
  if (status === "partial") return "Review threads: the list from GitHub is incomplete";
  if (status === "not_checked") return "Review threads: not checked";
  return null;
}

/** Note for a pending-review list that GitHub has more comments for than were read. */
export const PENDING_INCOMPLETE = "More pending comments exist on GitHub than are listed here.";
