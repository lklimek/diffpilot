import type { CSSProperties } from "react";
import type { Annotation, LinesResponse } from "../../api/generated/types";
import { Tooltip } from "../tooltip/Tooltip";

interface CodeViewProps {
  /** Lines of the range with their deterministic gutter facts. */
  lines: LinesResponse;
  /** LLM annotations of the step; their ranges are highlighted. */
  annotations: Annotation[];
}

interface Row {
  number: number;
  text: string;
  changed: boolean;
}

interface Run {
  rows: Row[];
  annotations: Annotation[];
}

// Consecutive rows covered by at least one annotation form one highlight; others stay plain.
function runsOf(rows: Row[], annotations: Annotation[]): Run[] {
  const runs: Run[] = [];
  for (const row of rows) {
    const covering = annotations.filter(
      (a) => a.range.start_line <= row.number && row.number <= a.range.end_line,
    );
    const last = runs.at(-1);
    const highlighted = covering.length > 0;
    if (last && last.annotations.length > 0 === highlighted) {
      last.rows.push(row);
      for (const a of covering) if (!last.annotations.includes(a)) last.annotations.push(a);
    } else {
      runs.push({ rows: [row], annotations: covering });
    }
  }
  return runs;
}

function Line({ row, width }: { row: Row; width: number }) {
  return (
    <div
      data-line={row.number}
      data-changed={row.changed || undefined}
      className="grid min-w-full grid-cols-[var(--ln)_28px_max-content]"
      style={{ "--ln": `${width + 2}ch` } as CSSProperties}
    >
      <span className="sticky left-0 pr-2.5 text-right text-muted select-none">{row.number}</span>
      <span className="flex items-center select-none">
        {row.changed && (
          <>
            <span aria-hidden className="inline-block h-[1.35em] w-1 bg-mark" />
            <span className="sr-only">changed line</span>
          </>
        )}
      </span>
      <code className="pr-4 whitespace-pre">{row.text}</code>
    </div>
  );
}

function WasIs({ annotations }: { annotations: Annotation[] }) {
  const pairs = [...new Map(annotations.map((a) => [`${a.was}\u0000${a.is}`, a])).values()];
  return (
    <span className="grid gap-1">
      {pairs.map((a) => (
        <span key={a.id}>
          Was: {a.was} → Is: {a.is}
        </span>
      ))}
    </span>
  );
}

/** Plain code view: numbered lines, neutral gutter mark on changed lines, one highlight on annotated lines. */
export function CodeView({ lines, annotations }: CodeViewProps) {
  const rows: Row[] = lines.lines.map((text, i) => ({
    number: lines.from + i,
    text,
    changed: lines.changed.includes(lines.from + i),
  }));
  const width = String(lines.to).length;
  const label = `${lines.path}, lines ${lines.from} to ${lines.to}`;
  return (
    <div
      role="region"
      aria-label={label}
      tabIndex={0}
      className="overflow-auto rounded-md border border-border bg-code-bg py-0.5 font-mono text-[12.5px] leading-[1.55]"
    >
      {lines.side === "base" && (
        <p className="sticky left-0 w-max px-2 font-sans text-[11px] text-muted">
          Removed code at the base commit
        </p>
      )}
      <div className="w-max min-w-full">
        {runsOf(rows, annotations).map((run) =>
          run.annotations.length === 0 ? (
            run.rows.map((row) => <Line key={row.number} row={row} width={width} />)
          ) : (
            <Tooltip
              key={run.rows[0]!.number}
              tip={<WasIs annotations={run.annotations} />}
              className="block"
            >
              <div
                role="mark"
                tabIndex={0}
                className="block min-w-full cursor-help border-l-[5px] border-accent bg-accent-bg text-text"
              >
                {run.rows.map((row) => (
                  <Line key={row.number} row={row} width={width} />
                ))}
              </div>
            </Tooltip>
          ),
        )}
      </div>
    </div>
  );
}
