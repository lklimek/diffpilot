import { CodeText } from "../section/CodeText";
import { Tooltip } from "../tooltip/Tooltip";
import { buttonClass } from "../button/button";
import { useId } from "react";
import type { Flow, StepId } from "../../api/generated/types";
import { ClassificationLabel, GroupLabel, RiskLabel } from "../badge/LlmLabel";
import { Section } from "../section/Section";
import { ReviewToggle } from "../step/ReviewToggle";
import { reviewedSteps } from "./progress";

interface FlowIntroBlockProps {
  /** The flow. */
  flow: Flow;
  now?: number;
  onRequest?: () => void;
  busy?: boolean;
  /** Flows this flow depends on (LLM claim), in tour order. */
  dependencies: Flow[];
  /** Steps the user ticked (any flow). */
  ticked: StepId[];
  /** Ticks (`true`) or unticks (`false`) every step of the flow; no button without it. */
  onSetReviewed?: (reviewed: boolean) => void;
}

const RISK_EDGE = { high: "border-l-high-bd", medium: "border-l-med-bd", low: "border-l-low-bd" } as const;
const TERM = "pt-0.5 text-[11px] font-semibold tracking-wide text-muted uppercase";

/** Flow introduction block: title, LLM risk, progress, Problem, Change, Before / After example. */
export function FlowIntroBlock({
  flow,
  dependencies,
  ticked,
  onSetReviewed,
  now,
  onRequest,
  busy,
}: FlowIntroBlockProps) {
  const headingId = useId();
  const { llm, facts } = flow;
  if (flow.analysis_status && flow.analysis_status.state !== "analysed") {
    const status = flow.analysis_status;
    const minutes =
      status.started_at && now !== undefined
        ? Math.max(0, Math.floor((now - Date.parse(status.started_at)) / 60000))
        : null;
    const text =
      status.state === "analysing"
        ? `Being analysed${minutes === null ? "" : ` · started ${minutes} min ago`}`
        : status.state === "queued"
          ? "Queued"
          : status.state === "failed"
            ? "Failed — Retry"
            : "Not analysed yet";
    return (
      <article aria-labelledby={headingId}>
        <h2 id={headingId}>
          {flow.position} {llm.title}
        </h2>
        <ClassificationLabel value={llm.classification} />
        <p>{flow.planned?.intent}</p>
        {flow.planned && <p>Plan rank: {flow.planned.importance_rank} (LLM)</p>}
        <p role="status">{text}</p>
        <p>These changes have not been assessed. The plan is an LLM grouping, not a finding of safety.</p>
        {status.message && <p>{status.message}</p>}
        <ul>
          {flow.planned?.ranges.map((r, i) => (
            <li key={i}>
              {r.file} ({r.side}): {r.ranges.map((n) => `${n.start_line}–${n.end_line}`).join(", ")}
            </li>
          ))}
        </ul>
        {onRequest && status.state !== "analysing" && (
          <Tooltip tip="Prioritises this flow after the running job; opening a pending flow also requests it.">
            <button
              type="button"
              aria-disabled={busy || undefined}
              className={buttonClass("plain")}
              onClick={() => !busy && onRequest()}
            >
              {status.state === "failed" ? "Retry this flow" : "Analyse this flow now"}
            </button>
          </Tooltip>
        )}
      </article>
    );
  }

  const reviewed = reviewedSteps(flow, ticked);
  const total = flow.steps.length;
  const rows: [string, string][] = [
    ["Problem (before this PR)", llm.introduction.problem_before],
    ["Change (what this PR does)", llm.introduction.change],
    ["Before / After example", llm.introduction.example_before_after],
  ];
  if (dependencies.length > 0) {
    rows.push(["Depends on", dependencies.map((d) => `Flow ${d.position}: ${d.llm.title}`).join("; ")]);
  }
  rows.push([
    "Steps",
    `${facts.step_count} ${facts.step_count === 1 ? "step" : "steps"} in ${facts.files.join(", ")}`,
  ]);
  return (
    <article aria-labelledby={headingId} className="grid gap-3.5">
      <header
        className={`flex flex-wrap items-start gap-3 rounded-lg border border-l-[6px] border-border ${llm.risk ? RISK_EDGE[llm.risk] : "border-l-mv-bd"} bg-surface px-4 py-3`}
      >
        <div className="min-w-0 flex-1">
          <h2 id={headingId} className="flex flex-wrap items-center gap-2 text-[17px] font-bold">
            <span className="font-mono text-[15px] text-muted">{flow.position}</span> {llm.title}
          </h2>
          <div className="mt-1 flex flex-wrap items-center gap-1.5">
            <RiskLabel risk={llm.risk} />
            <ClassificationLabel value={llm.classification} />
            <GroupLabel group={flow.group} />
          </div>
        </div>
        <div className="flex items-center gap-2.5">
          <span className="text-[12.5px] text-muted">
            {reviewed} of {total} {total === 1 ? "step" : "steps"} reviewed
          </span>
          {onSetReviewed && (
            <ReviewToggle
              label="Mark flow reviewed"
              tip="Ticks all steps of this flow as reviewed by you; press again to untick them."
              pressed={total > 0 && reviewed === total}
              onChange={onSetReviewed}
            />
          )}
        </div>
      </header>
      <Section title="LLM analysis" tone="llm">
        <dl className="m-0 grid grid-cols-1 gap-x-3 gap-y-1.5 text-[13px] sm:grid-cols-[minmax(10rem,auto)_1fr]">
          {rows
            .filter(([, value]) => value.trim())
            .map(([term, value]) => (
              <div key={term} className="contents">
                <dt className={TERM}>{term}</dt>
                <dd className="m-0 max-w-[100ch]">
                  <CodeText text={value} />
                </dd>
              </div>
            ))}
        </dl>
      </Section>
    </article>
  );
}
