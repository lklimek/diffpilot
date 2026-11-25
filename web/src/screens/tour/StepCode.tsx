import { useRef, useState, type ComponentProps } from "react";
import { useQueries } from "@tanstack/react-query";
import { ApiError } from "../../api/client";
import type { LineRange, Step } from "../../api/generated/types";
import type { StepFactsWithLeadingContext } from "../../api/provisional/stepFacts";
import { linesQuery, useLines } from "../../api/queries";
import { inApp } from "../../components/inApp";
import { CONTEXT_ACTION, ContextFold } from "../../components/code-view/ContextFold";
import { Tooltip } from "../../components/tooltip/Tooltip";
import { StatusMessage } from "../../components/status/StatusMessage";
import { errorText, LinesCode } from "./code";

type Direction = "above" | "below";
type Neighbour = { step: Step; label: string; href: string };

function neighbours(step: Step, siblings: readonly Neighbour[]) {
  const { file, side, range } = step.facts;
  const others = siblings.filter(
    (other) => other.step.id !== step.id && other.step.facts.file === file && other.step.facts.side === side,
  );
  const start = (other: Neighbour) => other.step.facts.range.start_line;
  const end = (other: Neighbour) => other.step.facts.range.end_line;
  return {
    above: others.filter((other) => start(other) < range.start_line).sort((a, b) => end(b) - end(a))[0],
    below: others.filter((other) => end(other) > range.end_line).sort((a, b) => start(a) - start(b))[0],
  };
}

const BATCH = 10;

function batch(direction: Direction, edge: number, limit: number, index: number): LineRange {
  const above = direction === "above";
  const near = edge + BATCH * index * (above ? -1 : 1);
  return above
    ? { start_line: Math.max(limit, near - BATCH + 1), end_line: near }
    : { start_line: near, end_line: Math.min(limit, near + BATCH - 1) };
}

function present(range: LineRange) {
  return range.start_line <= range.end_line;
}

function useContextBatches(
  { analysisId }: ComponentProps<typeof LinesCode>["ctx"],
  { file, side }: Step["facts"],
  direction: Direction,
  edge: number,
  limit: number,
  neighbour: Neighbour | undefined,
  onStep: (id: string) => void,
  enabled: boolean,
) {
  const [count, setCount] = useState(0);
  const expand = useRef<HTMLButtonElement>(null);
  const make = (i: number) => batch(direction, edge, limit, i);
  const ranges = enabled ? Array.from({ length: count }, (_, i) => make(i)).filter(present) : [];
  const results = useQueries({
    queries: ranges.map((range) =>
      linesQuery(analysisId, { path: file, side, from: range.start_line, to: range.end_line }),
    ),
  });
  const rows = results.flatMap((q) => (q.data ? [{ lines: q.data, kind: "expanded" as const }] : []));
  if (direction === "above") rows.reverse();

  const failed = results.find((q) => q.isError);
  const unavailable = failed?.error instanceof ApiError && [404, 501].includes(failed.error.status);
  const loading = results.some((q) => q.isFetching);
  const next = make(count);
  const more = present(next) && !unavailable;
  const showNeighbour = !present(next) && neighbour;
  const control = enabled && (more || count > 0 || neighbour) && (
    <ContextFold direction={direction}>
      {(more || count > 0) && (
        <Tooltip
          tip={more ? "Shows the next lines from the analysed commit here." : "Collapses the extra lines."}
        >
          <button
            type="button"
            ref={expand}
            className={CONTEXT_ACTION}
            aria-disabled={loading || undefined}
            onClick={() => {
              if (loading) return;
              if (failed && !unavailable) void failed.refetch();
              else setCount((n) => (more ? n + 1 : 0));
            }}
          >
            {failed && !unavailable ? (
              "Retry loading context"
            ) : more ? (
              <>
                Show {next.end_line - next.start_line + 1} more{" "}
                {next.start_line === next.end_line ? "line" : "lines"} {direction} ({next.start_line}–
                {next.end_line})
              </>
            ) : (
              "Hide added lines"
            )}
          </button>
        </Tooltip>
      )}
      {count > 0 && (more || (failed && !unavailable)) && (
        <Tooltip tip="Collapses the extra lines.">
          <button
            type="button"
            className={CONTEXT_ACTION}
            onClick={() => {
              setCount(0);
              expand.current?.focus();
            }}
          >
            Hide added lines
          </button>
        </Tooltip>
      )}
      {showNeighbour && (
        <a
          href={neighbour.href}
          className={CONTEXT_ACTION}
          onClick={(event) => inApp(event, () => onStep(neighbour.step.id))}
        >
          {neighbour.label} continues {direction}
        </a>
      )}
      {loading && <span>Loading lines…</span>}
      {failed && (
        <StatusMessage kind="error">
          {unavailable
            ? "Context is unavailable on this server."
            : `Cannot show context: ${errorText(failed.error)}`}
        </StatusMessage>
      )}
    </ContextFold>
  );
  return { rows, control };
}

export function StepCode({
  step,
  siblings,
  onStep,
  lines,
  ctx,
  ...props
}: ComponentProps<typeof LinesCode> & {
  step: Step;
  siblings: readonly Neighbour[];
  onStep: (id: string) => void;
}) {
  const facts: StepFactsWithLeadingContext = step.facts;
  const { file, side, range, leading_context: leading } = facts;
  const [showLeading, setShowLeading] = useState(true);
  const { above, below } = neighbours(step, siblings);
  const first = Math.max(1, (above?.step.facts.range.end_line ?? 0) + 1);
  const last = Math.min(
    lines.data?.total_lines ?? range.end_line,
    (below?.step.facts.range.start_line ?? Infinity) - 1,
  );
  const doc =
    leading && leading.start_line >= first && leading.end_line === range.start_line - 1 && present(leading)
      ? leading
      : null;
  const docs = useLines(
    ctx.analysisId,
    showLeading && doc && lines.data
      ? { path: file, side, from: doc.start_line, to: doc.end_line }
      : undefined,
  );
  const before = useContextBatches(
    ctx,
    facts,
    "above",
    (doc?.start_line ?? range.start_line) - 1,
    first,
    above,
    onStep,
    !!lines.data,
  );
  const after = useContextBatches(ctx, facts, "below", range.end_line + 1, last, below, onStep, !!lines.data);
  const leadingControl = doc && (
    <ContextFold direction="above">
      <span>
        doc comment and attributes · {doc.start_line}–{doc.end_line}
      </span>
      <Tooltip tip="Shows or hides the doc comment and attributes from the analysed commit.">
        <button
          type="button"
          className={CONTEXT_ACTION}
          aria-label={`${showLeading ? "Hide" : "Show"} doc comment and attributes`}
          aria-expanded={showLeading}
          onClick={() => setShowLeading((show) => !show)}
        >
          {showLeading ? "Hide" : "Show"}
        </button>
      </Tooltip>
      {showLeading && docs.isError && (
        <StatusMessage kind="error">
          Cannot show the doc comment and attributes: {errorText(docs.error)}
        </StatusMessage>
      )}
    </ContextFold>
  );
  return (
    <LinesCode
      {...props}
      ctx={ctx}
      lines={lines}
      before={
        showLeading && docs.data ? [...before.rows, { lines: docs.data, kind: "leading" }] : before.rows
      }
      after={after.rows}
      beforeControl={before.control}
      afterControl={after.control}
      leadingControl={leadingControl}
    />
  );
}
