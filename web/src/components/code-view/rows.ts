import type { Annotation, DeletedBlock, LinesResponse } from "../../api/generated/types";

/** One displayed line with its deterministic facts. */
export interface Row {
  /** Line number on the response's side. */
  line: number;
  /** Index into `LinesResponse.lines`. */
  index: number;
  text: string;
  changed: boolean;
  moved: boolean;
  /** Moved line that differs from the moved original (`≠`). */
  differs: boolean;
}

/** Plain rows, an LLM highlight over rows, or removed code shown at its marker. */
export type Segment =
  | { kind: "rows"; rows: Row[] }
  | { kind: "highlight"; rows: Row[]; annotations: Annotation[] }
  | { kind: "deleted"; block: DeletedBlock; paired: Row[]; annotations: Annotation[] };

const covers = (a: Annotation, line: number) => a.range.start_line <= line && line <= a.range.end_line;

/**
 * Splits the displayed lines into segments: annotations that overlap form one highlight, adjacent disjoint ones
 * stay apart; deleted blocks sit after their anchor line (0 = before the first) and split a highlight.
 */
export function buildSegments(lines: LinesResponse, annotations: readonly Annotation[]): Segment[] {
  const changed = new Set(lines.changed);
  const segments: Segment[] = [];
  const deletedAfter = (line: number) =>
    lines.deleted
      .filter((d) => d.after_line === line)
      .forEach((block) => segments.push({ kind: "deleted", block, paired: [], annotations: [] }));

  deletedAfter(lines.from - 1);
  lines.lines.forEach((text, index) => {
    const line = lines.from + index;
    const moves = lines.moved.filter((m) => m.range.start_line <= line && line <= m.range.end_line);
    const row: Row = {
      line,
      index,
      text,
      changed: changed.has(line),
      moved: moves.length > 0,
      differs: moves.some((m) => m.differing.includes(line)),
    };
    const covering = annotations.filter((a) => covers(a, line));
    const last = segments.at(-1);
    if (
      last?.kind === "deleted" &&
      last.block.replacement &&
      line >= last.block.replacement.start_line &&
      line <= last.block.replacement.end_line
    ) {
      last.paired.push(row);
      for (const a of covering) if (!last.annotations.includes(a)) last.annotations.push(a);
      deletedAfter(line);
      return;
    }
    if (covering.length === 0) {
      if (last?.kind === "rows") last.rows.push(row);
      else segments.push({ kind: "rows", rows: [row] });
    } else if (last?.kind === "highlight" && covering.some((a) => last.annotations.includes(a))) {
      last.rows.push(row);
      for (const a of covering) if (!last.annotations.includes(a)) last.annotations.push(a);
    } else {
      segments.push({ kind: "highlight", rows: [row], annotations: covering });
    }
    deletedAfter(line);
  });
  return segments;
}
