import { useId, type ReactNode } from "react";
import type { Assessment, Step } from "../../api/generated/types";
import { DifferingLinesBadge, ProvenanceBadge } from "../badge/Badge";
import { KindLabel, RiskLabel } from "../badge/LlmLabel";
import { evidenceText, rangeText } from "../section/format";
import { LlmLine, Section, Subhead } from "../section/Section";
import { Tooltip } from "../tooltip/Tooltip";

interface StepBlockProps {
  /** The step. */
  step: Step;
  /** The code of the step (code view, or its loading / error state). */
  code: ReactNode;
}

const ORIGIN = {
  introduced: "introduced by this PR",
  pre_existing: "pre-existing",
  unclear: "unclear",
} as const;
const BLOCKING = { blocks_this_pr: "blocks this PR", follow_up: "follow-up" } as const;

function AssessmentBody({ assessment }: { assessment: Assessment }) {
  const rows: [string, ReactNode][] = [
    ["What a user or operator would notice", assessment.headline],
    [
      "Risk at system scale",
      <>
        <RiskLabel risk={assessment.risk} /> {assessment.reason}
      </>,
    ],
    ["Basis", assessment.basis_note],
    ["Not checked", assessment.not_checked],
    ["Origin", ORIGIN[assessment.origin]],
    ["Merge impact", BLOCKING[assessment.blocking]],
    ["Smallest correction", assessment.smallest_correction ?? "none given"],
  ];
  return (
    <dl className="m-0 mt-1">
      {rows.map(([term, value]) => (
        <div key={term}>
          <dt className="mt-1.5 text-[10.5px] font-bold tracking-wide text-muted uppercase">{term}</dt>
          <dd className="m-0">{value}</dd>
        </div>
      ))}
    </dl>
  );
}

/** Step block: header, deterministic facts, the four LLM facts, code slot, questions, collapsed assessment. */
export function StepBlock({ step, code }: StepBlockProps) {
  const headingId = useId();
  const questionsId = useId();
  const { facts, llm } = step;
  const changed = facts.changed_lines.reduce((n, r) => n + r.end_line - r.start_line + 1, 0);
  const questions = llm.annotations.filter((a) => a.type === "question");
  const fourFacts: [string, string][] = [
    ["Called by", llm.called_by],
    ["Decides or changes", llm.decides],
    ["Goes next", llm.next],
    ["Why", llm.why],
  ];
  return (
    <article aria-labelledby={headingId} className="@container rounded-lg border border-border bg-surface">
      <header className="flex flex-wrap items-center gap-2 border-b border-border px-3 py-2">
        <h2 id={headingId} className="flex items-center gap-2 text-[15px] font-bold">
          <span className="inline-flex h-[22px] min-w-[22px] items-center justify-center rounded-full bg-text px-1 font-mono text-xs text-bg">
            {step.number}
          </span>{" "}
          {llm.title}
        </h2>
        <KindLabel kind={llm.kind} />
        <code className="font-semibold">{facts.file}</code>
        {llm.symbol && <span className="text-muted">{llm.symbol}</span>}
        <Tooltip tip={`Open these lines on GitHub at the ${facts.side} commit (new tab)`}>
          <a href={facts.github_url} target="_blank" rel="noopener noreferrer" className="font-mono text-xs">
            L{rangeText(facts.range)} ↗
          </a>
        </Tooltip>
      </header>
      {/* Wide (21:9): facts and LLM text on the left, the code beside them (mockup .stepgrid). */}
      <div className="grid content-start gap-3 p-3 @[1300px]:grid-cols-[minmax(380px,560px)_minmax(0,1fr)]">
        <Section title="Facts" subtitle="From the diff; no LLM.">
          <p className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[12.5px]">
            <span>
              Lines {rangeText(facts.range)}
              {facts.side === "base" && " at the base commit (removed code)"}
            </span>
            <span>
              {changed} changed {changed === 1 ? "line" : "lines"}
            </span>
            {facts.moved && (
              <>
                <span>
                  Moved from {facts.moved.from.file}:{rangeText(facts.moved.from.range)}
                </span>
                {facts.moved.badge ? (
                  <ProvenanceBadge badge={facts.moved.badge} />
                ) : (
                  <DifferingLinesBadge count={facts.moved.differing_lines} />
                )}
              </>
            )}
          </p>
        </Section>
        <Section title="LLM analysis" tone="llm">
          <LlmLine />
          <dl className="m-0 grid grid-cols-[auto_1fr] gap-x-2.5 gap-y-0.5 text-[12.5px]">
            {fourFacts.map(([term, value]) => (
              <div key={term} className="contents">
                <dt className="pt-0.5 text-[10.5px] font-bold tracking-wide text-muted uppercase">{term}</dt>
                <dd className={`m-0 max-w-[100ch] ${term === "Why" ? "font-semibold" : ""}`}>{value}</dd>
              </div>
            ))}
          </dl>
        </Section>
        <div className="min-w-0 @[1300px]:col-start-2 @[1300px]:row-span-3 @[1300px]:row-start-1">{code}</div>
        <Section title="Questions and assessment" tone="llm">
          <LlmLine />
          <Subhead>
            <span id={questionsId}>Questions to check</span>
          </Subhead>
          {questions.length === 0 ? (
            <p className="text-[12.5px]">The LLM raised no questions. That is not evidence of no defects.</p>
          ) : (
            <ul aria-labelledby={questionsId} className="m-0 list-disc pl-5 text-[12.5px]">
              {questions.map((q) => (
                <li key={q.id}>
                  {q.text}{" "}
                  <span className="text-muted">
                    ({q.basis === "fact" ? "fact" : "inference"}
                    {q.evidence.length > 0 && `; ${q.evidence.map(evidenceText).join(", ")}`})
                  </span>
                </li>
              ))}
            </ul>
          )}
          <details className="mt-2 border-t border-dashed border-border pt-1 text-[12.5px]">
            <summary className="cursor-pointer font-semibold">Assessment (LLM)</summary>
            {llm.assessment ? (
              <AssessmentBody assessment={llm.assessment} />
            ) : (
              <p className="mt-1">The LLM gave no assessment for this step.</p>
            )}
          </details>
        </Section>
      </div>
    </article>
  );
}
