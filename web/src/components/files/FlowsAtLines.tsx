import { useId, type ReactNode } from "react";
import type { AnalysisState, FlowId, HunkPart, LineRange, StepId } from "../../api/generated/types";
import { ProvenanceBadge } from "../badge/Badge";
import { RiskLabel } from "../badge/LlmLabel";
import { FlowWorkLabel } from "../flow-intro/status";
import { NOT_ANALYSED_LABEL } from "../quiet/labels";
import { CodeText } from "../section/CodeText";
import { rangeText } from "../section/format";
import { ProvenanceTag } from "../section/ProvenanceTag";
import { Tooltip } from "../tooltip/Tooltip";
import type { FlowAtLines, StepAtLines } from "./model";

interface FlowsAtLinesProps {
  /** The flows at the lines (`flowsAt`), in tour order. */
  flows: readonly FlowAtLines[];
  /** Steps ticked in the Flows view; shown read-only (ticking happens only there). */
  ticked: readonly StepId[];
  analysis: AnalysisState;
  /** Narrows the Inspector to one step; none for the step already narrowed to. */
  onDetails?: (step: StepId) => void;
  onOpenInFlows: (step: StepId) => void;
  /** Requests the analysis of a flow not analysed yet; no button without it. */
  onRequestFlow?: (flow: FlowId) => void;
  /** A flow request is being sent. */
  requestBusy?: boolean;
}

const BUTTON =
  "min-h-6 cursor-pointer rounded border border-border bg-surface px-2 text-[12px] text-text hover:border-accent aria-disabled:cursor-wait aria-disabled:opacity-60";

const rangesText = (ranges: readonly LineRange[], prefix: string) =>
  ranges.length === 0
    ? []
    : [
        `${prefix}${ranges.length === 1 && ranges[0]!.start_line === ranges[0]!.end_line ? "line" : "lines"} ${ranges.map(rangeText).join(", ")}`,
      ];
/** "lines 96, 100–103 · removed lines 102–103": head and base numbers are of different commits. */
const linesText = (at: StepAtLines) =>
  [...rangesText(at.lines, ""), ...rangesText(at.removed, "removed ")].join(" · ");

function StepRow({
  at,
  ticked,
  onDetails,
  onOpenInFlows,
}: { at: StepAtLines; ticked: boolean } & Pick<FlowsAtLinesProps, "onDetails" | "onOpenInFlows">) {
  const { step } = at;
  const remarks = step.llm.annotations.length;
  const name = `Step ${step.number} · ${step.llm.title}`;
  return (
    <li className="grid gap-0.5 border-t border-border pt-1.5">
      <span className="text-[10.5px] font-bold tracking-wide text-muted uppercase">
        {at.role === "explained" ? "Explained in" : "Also shown in"}
      </span>
      <span className="flex flex-wrap items-center gap-1.5">
        <b>Step {step.number}</b> {step.llm.title} <RiskLabel risk={step.llm.assessment?.risk ?? null} />
      </span>
      <span className="text-[12px] text-muted">
        {linesText(at)}
        {at.role === "also" && ": shown as context; another part explains them"}
        {remarks > 0 && (
          <span className="text-llm">
            {" "}
            <span aria-hidden>{"◆".repeat(Math.min(remarks, 5))}</span>
            <span className="sr-only">{`${remarks} LLM ${remarks === 1 ? "remark" : "remarks"}`}</span>
          </span>
        )}
      </span>
      {step.llm.decides.trim() && (
        <span>
          <CodeText text={step.llm.decides} /> <ProvenanceTag source="llm" />
        </span>
      )}
      <span className="flex flex-wrap items-center gap-2">
        <Tooltip tip="Your tick of this step in the Flows view; steps are ticked only there.">
          <span
            tabIndex={0}
            className={`cursor-help text-[12px] ${ticked ? "font-semibold text-ok" : "text-muted"}`}
          >
            {ticked ? "☑ ticked in Flows" : "☐ not ticked in Flows"}
          </span>
        </Tooltip>
        {onDetails && (
          <Tooltip tip="Shows this step's behaviour, why, consequences, flags and assessment in the Inspector.">
            <button
              type="button"
              aria-label={`Details: ${name}`}
              onClick={() => onDetails(step.id)}
              className={BUTTON}
            >
              Details
            </button>
          </Tooltip>
        )}
        <Tooltip tip="Opens this step in the Flows view.">
          <button
            type="button"
            aria-label={`Open in Flows: ${name}`}
            onClick={() => onOpenInFlows(step.id)}
            className={BUTTON}
          >
            Open in Flows →
          </button>
        </Tooltip>
      </span>
    </li>
  );
}

/**
 * Every flow at some lines with its steps there: Explained in / Also shown in, LLM rating, decision, the tick
 * state of the Flows view (read-only), Details and Open in Flows. A flow not analysed yet says so and offers its
 * analysis.
 */
export function FlowsAtLines({
  flows,
  ticked,
  analysis,
  onDetails,
  onOpenInFlows,
  onRequestFlow,
  requestBusy = false,
}: FlowsAtLinesProps) {
  const id = useId();
  return (
    <div className="grid gap-2">
      {flows.map(({ flow, planned, steps }) => {
        const pending = flow.analysis_status !== undefined && flow.analysis_status.state !== "analysed";
        const intro = flow.llm.introduction;
        return (
          <div
            key={flow.id}
            role="group"
            aria-labelledby={`${id}-${flow.id}`}
            className="grid gap-1 rounded-md border border-border bg-surface px-2.5 py-1.5"
          >
            <span id={`${id}-${flow.id}`} className="flex flex-wrap items-center gap-1.5">
              <b>Flow {flow.position}</b> {flow.llm.title}
              {pending && flow.analysis_status ? (
                <FlowWorkLabel status={flow.analysis_status} analysis={analysis} />
              ) : (
                <RiskLabel risk={flow.llm.risk} />
              )}
            </span>
            {!pending && (intro.problem_before.trim() || intro.change.trim()) && (
              <span className="text-[12.5px]">
                <span className="text-muted">Why this flow: </span>
                <CodeText text={intro.problem_before} /> → <CodeText text={intro.change} />{" "}
                <ProvenanceTag source="llm" />
              </span>
            )}
            {pending && planned && (
              <>
                <span className="text-[12.5px]">
                  <b>not analysed yet</b>
                  {flow.planned?.intent && (
                    <>
                      {" · "}
                      <CodeText text={flow.planned.intent} /> <ProvenanceTag source="llm" />
                    </>
                  )}
                </span>
                <span className="text-[12px] text-muted">
                  Its steps, remarks and flags appear here when the flow is analysed. Until then the code
                  shows the changed lines only.
                </span>
                {onRequestFlow && flow.analysis_status?.state === "not_analysed" && (
                  <Tooltip tip="Analyse this flow: starts LLM work (one agent run) for this flow.">
                    <button
                      type="button"
                      aria-disabled={requestBusy || undefined}
                      onClick={() => !requestBusy && onRequestFlow(flow.id)}
                      className={`${BUTTON} justify-self-start`}
                    >
                      ▶ Analyse this flow
                    </button>
                  </Tooltip>
                )}
              </>
            )}
            {steps.length > 0 && (
              <ul className="m-0 grid list-none gap-1.5 p-0">
                {steps.map((at) => (
                  <StepRow
                    key={at.step.id}
                    at={at}
                    ticked={ticked.includes(at.step.id)}
                    onDetails={onDetails}
                    onOpenInFlows={onOpenInFlows}
                  />
                ))}
              </ul>
            )}
          </div>
        );
      })}
    </div>
  );
}

const linesOf = (part: HunkPart) => {
  const n = part.range.end_line - part.range.start_line + 1;
  return `${part.side === "base" ? "removed " : ""}${n === 1 ? "line" : "lines"} ${rangeText(part.range)}`;
};

/** Changed lines in no flow, stated factually with their deterministic reason (never as safe). */
export function NoFlowPart({ part }: { part: HunkPart }) {
  const { owner } = part;
  const n = part.range.end_line - part.range.start_line + 1;
  let text: ReactNode;
  switch (owner.kind) {
    case "unexplained":
      text = (
        <>
          <b className="text-high">In no flow. Unexplained — read these yourself.</b>{" "}
          {owner.reason === "too_large_for_prompt"
            ? "These lines were not sent to the LLM: the file or flow was too large for a prompt."
            : `The analysis did not explain these ${n} changed ${n === 1 ? "line" : "lines"}; there is no LLM text for them.`}
        </>
      );
      break;
    case "not_analysed":
      text = (
        <>
          In no flow. Not analysed: recognised by file path (rule <code>{owner.rule}</code>){" "}
          <ProvenanceBadge badge={owner.badge} />, no LLM ({NOT_ANALYSED_LABEL[owner.category]}). The agent
          may read it as evidence; it does not explain it.
        </>
      );
      break;
    case "context_rule":
      text = (
        <>
          In no flow. Covered by a line rule (<code>{owner.rule}</code>){" "}
          <ProvenanceBadge badge={owner.badge} />: sent to the LLM as context, with no duty to explain it.
          Hidden in the code by default.
        </>
      );
      break;
    case "identical_move":
      text = (
        <>
          In no flow. Moved with identical text: from {owner.from.file}:{rangeText(owner.from.range)} to{" "}
          {owner.to.file}:{rangeText(owner.to.range)}. <ProvenanceBadge badge="text_identical" /> The check
          compares text only; it says nothing about behaviour at the new place.
        </>
      );
      break;
    case "whitespace_only":
      text = <>In no flow. Only whitespace changed; the code view shows these lines as unchanged.</>;
      break;
    default:
      return null;
  }
  return (
    <p data-part={owner.kind} className="m-0 text-[12.5px]">
      <span className="text-muted">{linesOf(part)}: </span>
      {text}
    </p>
  );
}
