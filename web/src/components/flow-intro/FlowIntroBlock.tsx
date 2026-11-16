import { useId } from "react";
import type { Flow } from "../../api/generated/types";
import { ClassificationLabel, GroupLabel, RiskLabel } from "../badge/LlmLabel";
import { LlmLine, Section } from "../section/Section";

interface FlowIntroBlockProps {
  /** The flow. */
  flow: Flow;
  /** Flows this flow depends on (LLM claim), in tour order. */
  dependencies: Flow[];
}

const RISK_EDGE = { high: "border-l-high-bd", medium: "border-l-med-bd", low: "border-l-low-bd" } as const;

/** Flow introduction block: title, LLM risk, Problem, Change, Before / After example, facts. */
export function FlowIntroBlock({ flow, dependencies }: FlowIntroBlockProps) {
  const headingId = useId();
  const { llm, facts } = flow;
  const rows: [string, string][] = [
    ["Problem (before this PR)", llm.introduction.problem_before],
    ["Change (what this PR does)", llm.introduction.change],
    ["Before / After example", llm.introduction.example_before_after],
  ];
  return (
    <article aria-labelledby={headingId} className="grid gap-3.5">
      <header
        className={`rounded-lg border border-l-[6px] border-border ${llm.risk ? RISK_EDGE[llm.risk] : "border-l-mv-bd"} bg-surface px-4 py-3`}
      >
        <h2 id={headingId} className="flex flex-wrap items-center gap-2 text-[17px] font-bold">
          <span className="font-mono text-[15px] text-muted">{flow.position}</span> {llm.title}
        </h2>
        <div className="mt-1 flex flex-wrap items-center gap-1.5">
          <RiskLabel risk={llm.risk} />
          <ClassificationLabel value={llm.classification} />
          <GroupLabel group={flow.group} />
        </div>
      </header>
      <Section title="LLM analysis" tone="llm">
        <LlmLine />
        <dl className="m-0 grid grid-cols-1 gap-x-3 gap-y-1.5 text-[13px] sm:grid-cols-[minmax(10rem,auto)_1fr]">
          {rows.map(([term, value]) => (
            <div key={term} className="contents">
              <dt className="pt-0.5 text-[11px] font-semibold tracking-wide text-muted uppercase">{term}</dt>
              <dd className="m-0 max-w-[100ch]">{value}</dd>
            </div>
          ))}
          {dependencies.length > 0 && (
            <div className="contents">
              <dt className="pt-0.5 text-[11px] font-semibold tracking-wide text-muted uppercase">
                Depends on
              </dt>
              <dd className="m-0">
                {dependencies.map((d) => `Flow ${d.position}: ${d.llm.title}`).join("; ")}
              </dd>
            </div>
          )}
        </dl>
      </Section>
      <Section title="Facts" subtitle="From the diff; no LLM.">
        <p className="mb-1">{facts.step_count} steps in these files:</p>
        <ul className="m-0 list-none p-0 font-mono text-xs">
          {facts.files.map((file) => (
            <li key={file}>{file}</li>
          ))}
        </ul>
      </Section>
    </article>
  );
}
