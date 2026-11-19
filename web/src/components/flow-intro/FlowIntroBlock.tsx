import { CodeText } from "../section/CodeText";
import { Tooltip } from "../tooltip/Tooltip";
import { buttonClass } from "../button/button";
import { useId } from "react";
import type { AnalysisState, Flow, StepId } from "../../api/generated/types";
import { ClassificationLabel, GroupLabel, RiskLabel } from "../badge/LlmLabel";
import { Section } from "../section/Section";
import { ReviewToggle } from "../step/ReviewToggle";
import { reviewedSteps } from "./progress";
import { flowWorkStatus } from "./status";
import { ANALYSE_AGAIN, ANALYSE_AGAIN_SENTENCE } from "./again";

/** The action a failed flow's server message names when the flow is analysed again from scratch. */
/** What analysing a flow again does to what was said about its steps (said before the click). */
const REPLACES_STEPS = "Replaces this flow's steps. Ticks, questions and feedback on them are removed.";

interface FlowIntroBlockProps {
  /** The flow. */
  flow: Flow;
  /** State of the whole analysis: a pending flow says what it waits for. */
  analysis?: AnalysisState;
  now?: number;
  onRequest?: () => void;
  busy?: boolean;
  /** Why the last request of this flow was refused. */
  error?: string;
  /** Flows this flow depends on (LLM claim), in tour order. */
  dependencies: Flow[];
  /** Steps the user ticked (any flow). */
  ticked: StepId[];
  /** Ticks (`true`) or unticks (`false`) every step of the flow; no button without it. */
  onSetReviewed?: (reviewed: boolean) => void;
}

const lineCount = (ranges: readonly { start_line: number; end_line: number }[]) =>
  ranges.reduce((n, r) => n + r.end_line - r.start_line + 1, 0);

const RISK_EDGE = { high: "border-l-high-bd", medium: "border-l-med-bd", low: "border-l-low-bd" } as const;
const TERM = "pt-0.5 text-[11px] font-semibold tracking-wide text-muted uppercase";

/** Flow introduction block: title, LLM risk, progress, Problem, Change, Before / After example. */
export function FlowIntroBlock({
  flow,
  analysis,
  dependencies,
  ticked,
  onSetReviewed,
  now,
  onRequest,
  busy,
  error,
}: FlowIntroBlockProps) {
  const headingId = useId();
  const { llm, facts } = flow;
  if (flow.analysis_status && flow.analysis_status.state !== "analysed") {
    const status = flow.analysis_status;
    const minutes =
      status.started_at && now !== undefined
        ? Math.max(0, Math.floor((now - Date.parse(status.started_at)) / 60000))
        : null;
    const work = flowWorkStatus(status, analysis);
    // A failed flow of a finished analysis, or one whose message names it, is analysed again from scratch.
    // TODO: replace the message match with a typed field of `FlowAnalysisStatus` (e.g. `replaces_steps: bool`)
    // once the contract has it.
    const again =
      status.state === "failed" &&
      (analysis?.state === "finished" || (status.message?.endsWith(ANALYSE_AGAIN_SENTENCE) ?? false));
    const text =
      work.text === "Being analysed" && minutes !== null
        ? `${work.text} · started ${minutes} min ago`
        : work.text;
    return (
      <article aria-labelledby={headingId} className="grid gap-3.5">
        <header className="rounded-lg border border-l-[6px] border-border border-l-mv-bd bg-surface px-4 py-3">
          <h2 id={headingId} className="flex flex-wrap items-center gap-2 text-[17px] font-bold">
            <span className="font-mono text-[15px] text-muted">{flow.position}</span> {llm.title}
          </h2>
          <div className="mt-1 flex flex-wrap items-center gap-1.5">
            <ClassificationLabel value={llm.classification} />
            <Tooltip tip={work.tip}>
              <span role="status" tabIndex={0} className="cursor-help text-[12.5px] font-semibold">
                {text}
              </span>
            </Tooltip>
          </div>
        </header>
        <Section title="LLM plan" tone="llm">
          <p className="text-[13px]">
            These changes have not been assessed. The plan is an LLM grouping, not a finding of safety.
          </p>
          {status.message && <p className="text-[13px]">{status.message}</p>}
          {flow.planned && (
            <p className="text-[13px] font-semibold">
              {new Set(flow.planned.ranges.map((r) => r.file)).size}{" "}
              {new Set(flow.planned.ranges.map((r) => r.file)).size === 1 ? "file" : "files"},{" "}
              {flow.planned.ranges.reduce((n, r) => n + lineCount(r.ranges), 0)} changed lines
            </p>
          )}
          {flow.planned && (
            <dl className="m-0 grid grid-cols-1 gap-x-3 gap-y-1.5 text-[13px] sm:grid-cols-[minmax(10rem,auto)_1fr]">
              <dt className={TERM}>Intent</dt>
              <dd className="m-0 max-w-[100ch]">
                <CodeText text={flow.planned.intent} />
              </dd>
              <dt className={TERM}>Plan rank</dt>
              <dd className="m-0">{flow.planned.importance_rank}</dd>
              <dt className={TERM}>Planned lines</dt>
              <dd className="m-0">
                <ul className="m-0 list-none p-0">
                  {flow.planned.ranges.map((r, i) => (
                    <li key={i}>
                      <code>{r.file}</code>: {r.side === "base" ? "removed lines" : "lines"}{" "}
                      {r.ranges.map((n) => `${n.start_line}–${n.end_line}`).join(", ")} ({lineCount(r.ranges)}{" "}
                      {lineCount(r.ranges) === 1 ? "line" : "lines"})
                    </li>
                  ))}
                </ul>
              </dd>
            </dl>
          )}
          {onRequest && status.state !== "analysing" && (
            <Tooltip
              tip={
                status.state === "queued"
                  ? "Moves this requested flow to the front: it runs after the job that is running now."
                  : again
                    ? REPLACES_STEPS
                    : status.state === "failed"
                      ? "Runs the LLM analysis of this flow again, after the job that is running now."
                      : "Runs the LLM analysis of this flow, after the job that is running now. Nothing runs until you ask."
              }
            >
              <button
                type="button"
                aria-disabled={busy || undefined}
                className={buttonClass(status.state === "queued" ? "plain" : "primary")}
                onClick={() => !busy && onRequest()}
              >
                {again
                  ? ANALYSE_AGAIN
                  : status.state === "failed"
                    ? "Retry this flow"
                    : status.state === "queued"
                      ? "Analyse this flow now"
                      : "Analyse this flow"}
              </button>
            </Tooltip>
          )}
          {error && <p role="alert">This flow was not requested: {error}</p>}
        </Section>
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
