import { useId, type ReactNode } from "react";
import type {
  CiStatus,
  FitTopic,
  PrContextBlock as PrContext,
  PrSummary,
  ProjectProfileStatus,
  RelatedWork,
  RelatedWorkKind,
} from "../../api/generated/types";
import { evidenceText } from "../section/format";
import { LlmLine, Section, Subhead } from "../section/Section";

/** The PR context as rendered. */
export type PrContextView = PrContext;

interface PrContextBlockProps {
  /** The pull request. */
  pr: PrSummary;
  /** Deterministic facts and LLM content of the PR context. */
  context: PrContextView;
}

const FIT_TOPIC: Record<FitTopic, string> = {
  layer_or_repository: "Right layer or repository?",
  size_for_problem: "Oversized for the problem?",
  duplicate_mechanism: "Duplicates an existing mechanism?",
  other: "Fit",
};

const RELATED_KIND: Record<RelatedWorkKind, string> = {
  duplicate: "Duplicate",
  competing_solution: "Competing solution",
  stacked_pr: "Stacked PR",
  linked_issue: "Linked issue",
  companion_repository: "Companion repository",
};

const CARD = "mb-1.5 rounded-md border border-border bg-surface-2 px-2 py-1.5 text-[12.5px]";

function Related({ item }: { item: RelatedWork }) {
  return (
    <li className={CARD}>
      {/* LLM text: never a link (threat model T12); only server-built GitHub links are links. */}
      <code className="break-all">{item.reference}</code>
      <span className="block text-[11px] text-muted">{RELATED_KIND[item.kind]}</span>
      {item.overlap}
    </li>
  );
}

function ciLine(ci: CiStatus): string {
  switch (ci.state) {
    case "failing":
      return `CI: ${ci.failing.length} of ${ci.total} checks failing`;
    case "passing":
      return `CI: all ${ci.total} checks passing`;
    case "pending":
      return `CI: ${ci.total} checks, still running`;
    case "none":
      return "CI: no checks reported";
    case "not_checked":
      return "CI: not checked";
  }
}

function profileLine(profile: ProjectProfileStatus): string {
  const unchecked = profile.memcan === "not_checked";
  if (profile.sources.length === 0) {
    return unchecked
      ? "No project profile file; a MemCan profile was not checked"
      : "No project profile — ratings are component-level";
  }
  const names = profile.sources.map((s) => (s === "repository_file" ? "repository file" : "MemCan"));
  return `Project profile: ${names.join(" and ")}${unchecked ? "; a MemCan profile was not checked" : ""}`;
}

function Fact({ children }: { children: ReactNode }) {
  return <li className="mb-1">{children}</li>;
}

/** PR context block: LLM analysis (problem, why, what it gives, related work, fit questions) and facts. */
export function PrContextBlock({ pr, context }: PrContextBlockProps) {
  const headingId = useId();
  const { facts, llm } = context;
  return (
    <article aria-labelledby={headingId} className="@container grid gap-3.5">
      <h2 id={headingId} className="text-xl font-bold">
        PR context: {pr.repo.owner}/{pr.repo.name} #{pr.number}
      </h2>
      {facts.agent_instruction_files.length > 0 && (
        <p role="note" className="rounded-md border border-l-4 border-med-bd bg-med-bg px-3 py-2">
          <strong>This PR changes files that look like agent instructions:</strong>{" "}
          <code>{facts.agent_instruction_files.join(", ")}</code>. The analysing agent may have read them, and
          they can steer its analysis. Review them like code and read the LLM analysis with that in mind.{" "}
          <span className="text-[12px] text-muted">From the diff; no LLM.</span>
        </p>
      )}
      <div className="grid grid-cols-1 items-start gap-3.5 @[1250px]:grid-cols-[minmax(0,3fr)_minmax(0,2fr)]">
        <Section title="LLM analysis" tone="llm">
          <LlmLine />
          <p>
            <strong>What the change gives:</strong> {llm.what_it_gives}
          </p>
          <p>
            <strong>Why it is needed:</strong> {llm.why_needed}
          </p>
          <p>
            <strong>Problem:</strong> {llm.problem}
          </p>
          <p className="mt-1 text-[12.5px] text-muted">
            The LLM split this PR into {facts.flow_count} flows with {facts.step_count} steps.
          </p>
          <Subhead>Related work</Subhead>
          {!facts.related_work_checked ? (
            <p>Related work: not checked</p>
          ) : llm.related_work.length === 0 ? (
            <p>The LLM found no related work.</p>
          ) : (
            <ul className="m-0 list-none p-0">
              {llm.related_work.map((item) => (
                <Related key={item.reference} item={item} />
              ))}
            </ul>
          )}
          <Subhead>Fit questions to check</Subhead>
          {llm.fit_questions.length === 0 ? (
            <p>The LLM raised no fit questions. That is not evidence of a good fit.</p>
          ) : (
            <ul className="m-0 list-none p-0">
              {llm.fit_questions.map((q) => (
                <li key={q.question} className={CARD}>
                  <span className="block text-[11px] text-muted">{FIT_TOPIC[q.topic]}</span>
                  {q.question}
                  {q.evidence.length > 0 && (
                    <span className="text-muted">
                      {" "}
                      Evidence: <code className="text-xs">{q.evidence.map(evidenceText).join(", ")}</code>
                    </span>
                  )}
                </li>
              ))}
            </ul>
          )}
        </Section>
        <Section title="Facts" subtitle="From GitHub and the diff; no LLM.">
          <ul className="m-0 list-none p-0">
            <Fact>
              {ciLine(facts.ci)}
              {facts.ci.failing.length > 0 && (
                <ul className="mt-1 list-disc pl-5">
                  {facts.ci.failing.map((check) => (
                    <li key={check.name}>
                      {check.url ? (
                        <a href={check.url} target="_blank" rel="noopener noreferrer">
                          {check.name}
                        </a>
                      ) : (
                        check.name
                      )}
                      {check.failing_tests.length > 0 && (
                        <>
                          : <code className="text-xs">{check.failing_tests.join(", ")}</code>
                        </>
                      )}
                    </li>
                  ))}
                </ul>
              )}
            </Fact>
            <Fact>{facts.counters.lines_removed_from_tests} lines removed from tests</Fact>
            <Fact>
              {facts.counters.dependencies_changed.length} dependencies changed
              {facts.counters.dependencies_changed.length > 0 &&
                `: ${facts.counters.dependencies_changed.join(", ")}`}
            </Fact>
            <Fact>{profileLine(facts.project_profile)}</Fact>
          </ul>
        </Section>
      </div>
    </article>
  );
}
