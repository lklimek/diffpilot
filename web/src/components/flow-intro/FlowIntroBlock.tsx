import { useId } from "react";
import type { Flow, StepId } from "../../api/generated/types";
import { ClassificationLabel, GroupLabel, RiskLabel } from "../badge/LlmLabel";
import { LlmLine, Section } from "../section/Section";
import { ReviewToggle } from "../step/ReviewToggle";
import { reviewedSteps } from "./progress";

interface FlowIntroBlockProps {
  /** The flow. */
  flow: Flow;
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
export function FlowIntroBlock({ flow, dependencies, ticked, onSetReviewed }: FlowIntroBlockProps) {
  const headingId = useId();
  const { llm, facts } = flow;
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
        <LlmLine />
        <dl className="m-0 grid grid-cols-1 gap-x-3 gap-y-1.5 text-[13px] sm:grid-cols-[minmax(10rem,auto)_1fr]">
          {rows.map(([term, value]) => (
            <div key={term} className="contents">
              <dt className={TERM}>{term}</dt>
              <dd className="m-0 max-w-[100ch]">{value}</dd>
            </div>
          ))}
        </dl>
      </Section>
    </article>
  );
}
