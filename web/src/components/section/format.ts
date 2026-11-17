import type { Counters, Evidence, LineRange } from "../../api/generated/types";

/** `file:line` of an evidence reference; base-side lines are marked. */
export const evidenceText = (e: Evidence) => `${e.file}:${e.line}${e.side === "base" ? " (base)" : ""}`;

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
