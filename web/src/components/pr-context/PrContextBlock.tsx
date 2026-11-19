import { GithubMark } from "../github/GithubMark";
import { isGithubUrl } from "../github/links";
import { Tooltip } from "../tooltip/Tooltip";
import { ModelsUsed } from "../usage/ModelsUsed";
import { UsageLine } from "../usage/UsageLine";
import type { UsageTotals } from "../../api/generated/types";
import { CHECKS_LINK_TIP, ciText } from "../section/ci";
import { MemcanStatusLine } from "../section/MemcanStatusLine";
import { CodeText } from "../section/CodeText";
import { plural, testLines } from "../quiet/labels";
import { EvidenceLinks } from "../section/EvidenceLinks";
import { useId, type ReactNode } from "react";
import type {
  Evidence,
  FitTopic,
  PrContextBlock as PrContext,
  PrSummary,
  ProjectProfileStatus,
  RelatedWork,
  RelatedWorkLink,
  RelatedWorkKind,
  ThreadsStatus,
} from "../../api/generated/types";
import { dependencyFacts, threadsNote } from "../section/format";
import { Section, Subhead } from "../section/Section";

/** The PR context as rendered. */
export type PrContextView = PrContext;

interface PrContextBlockProps {
  /** The pull request. */
  pr: PrSummary;
  /** Deterministic facts and LLM content of the PR context. */
  context: PrContextView;
  /** Review threads of the PR as listed (`Tour.threads`); read with `facts.threads_status`. */
  threadCount: number;
  usage?: UsageTotals | null;
  modelsUsed?: import("../../api/generated/types").ModelChoice[];
  onEvidence?: (evidence: Evidence) => void;
  /** The PR's checks page on GitHub; the CI summary links to it (new tab) when given. */
  checksHref?: string;
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

function Related({ item, target }: { item: RelatedWork; target?: RelatedWorkLink }) {
  const safe =
    target &&
    /^[a-zA-Z0-9-]+\/[a-zA-Z0-9_.-]+$/.test(target.repo) &&
    !target.repo.split("/").some((s) => s === "." || s === "..") &&
    Number.isSafeInteger(target.number) &&
    target.number > 0 &&
    ["pull_request", "issue"].includes(target.kind);
  const href = safe
    ? `https://github.com/${target.repo}/${target.kind === "pull_request" ? "pull" : "issues"}/${target.number}`
    : undefined;
  return (
    <li className={CARD}>
      {href && target ? (
        <a href={href} target="_blank" rel="noopener noreferrer">
          <code>
            {target.repo}#{target.number}
          </code>
          <GithubMark />
        </a>
      ) : (
        <code className="break-all">{item.reference}</code>
      )}
      {(!safe || !target?.verified) && (
        <span className="block text-muted">Target not checked against GitHub.</span>
      )}
      <span className="block text-[11px] text-muted">{RELATED_KIND[item.kind]}</span>
      <CodeText text={item.overlap} />
    </li>
  );
}

function profileLines(profile: ProjectProfileStatus): string[] {
  const unchecked = profile.memcan === "not_checked";
  const names = profile.sources.map((s) => (s === "repository_file" ? "repository file" : "MemCan"));
  const sources =
    names.length > 0
      ? `Project profile: ${names.join(" and ")}`
      : unchecked
        ? "No project profile file in the repository"
        : "No project profile — ratings are component-level";
  return unchecked ? [sources, "Project profile in MemCan: not checked"] : [sources];
}

function threadsFact(status: ThreadsStatus, count: number): string {
  const note = threadsNote(status);
  const listed = `${count} review ${count === 1 ? "thread" : "threads"}`;
  if (status === "not_checked") return note!;
  return note ? `${listed} listed. ${note}` : `${listed} on GitHub`;
}

function Fact({ children }: { children: ReactNode }) {
  return <li className="mb-1">{children}</li>;
}

/** PR context block: LLM analysis (problem, why, what it gives, related work, fit questions) and facts. */
export function PrContextBlock({
  pr,
  context,
  threadCount,
  onEvidence,
  usage,
  modelsUsed,
  checksHref,
}: PrContextBlockProps) {
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
          <div>
            <strong>Problem:</strong> <CodeText text={llm.problem} />
          </div>
          <div>
            <strong>Why it is needed:</strong> <CodeText text={llm.why_needed} />
          </div>
          <div>
            <strong>What the change gives:</strong> <CodeText text={llm.what_it_gives} />
          </div>
          <p className="mt-1 text-[12.5px] text-muted">
            The LLM split this PR into {plural(facts.flow_count, "flow")} with{" "}
            {plural(facts.step_count, "step")}.
          </p>
          {(!facts.related_work_checked || llm.related_work.length > 0) && <Subhead>Related work</Subhead>}
          {!facts.related_work_checked ? (
            <p>Related work: not checked</p>
          ) : llm.related_work.length === 0 ? null : (
            <ul className="m-0 list-none p-0">
              {llm.related_work.map((item) => (
                <Related
                  key={item.reference}
                  item={item}
                  target={facts.related_work_links?.find((link) => link.reference === item.reference)}
                />
              ))}
            </ul>
          )}
          {llm.fit_questions.length > 0 && <Subhead>Fit questions to check</Subhead>}
          {llm.fit_questions.length === 0 ? null : (
            <ul className="m-0 list-none p-0">
              {llm.fit_questions.map((q) => (
                <li key={q.question} className={CARD}>
                  <span className="block text-[11px] text-muted">{FIT_TOPIC[q.topic]}</span>
                  {q.evidence[0] && onEvidence ? (
                    <Tooltip tip="Open the PR source lines this fit question concerns.">
                      <button
                        type="button"
                        className="cursor-pointer text-left text-accent underline"
                        onClick={() => onEvidence(q.evidence[0]!)}
                      >
                        <CodeText text={q.question} />{" "}
                        <span className="text-xs">
                          {q.evidence[0].file}:{q.evidence[0].line}
                        </span>
                      </button>
                    </Tooltip>
                  ) : (
                    // TODO: citation-free fit questions need structured evidence before linking to code.
                    <CodeText text={q.question} />
                  )}
                  {q.evidence.length > 0 && (
                    <div className="text-muted">
                      {" "}
                      <EvidenceLinks evidence={q.evidence} onEvidence={onEvidence} />
                    </div>
                  )}
                </li>
              ))}
            </ul>
          )}
        </Section>
        <Section title="Facts" subtitle="From GitHub and the diff; no LLM.">
          <ul className="m-0 list-none p-0">
            <Fact>
              {checksHref ? (
                <Tooltip tip={CHECKS_LINK_TIP}>
                  <a href={checksHref} target="_blank" rel="noopener noreferrer">
                    {ciText(facts.ci)}
                    <GithubMark />
                  </a>
                </Tooltip>
              ) : (
                ciText(facts.ci)
              )}
              {facts.ci.failing.length > 0 && (
                <ul className="mt-1 list-disc pl-5">
                  {facts.ci.failing.map((check) => (
                    <li key={check.name}>
                      {check.url ? (
                        <a href={check.url} target="_blank" rel="noopener noreferrer">
                          {check.name}
                          {isGithubUrl(check.url) && <GithubMark />}
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
            <Fact>{threadsFact(facts.threads_status, threadCount)}</Fact>
            <Fact>{testLines(facts.counters)}</Fact>
            <Fact>
              <MemcanStatusLine status={facts.memcan_status} />
            </Fact>
            {dependencyFacts(facts.counters).map((line) => (
              <Fact key={line}>{line}</Fact>
            ))}
            {profileLines(facts.project_profile).map((line) => (
              <Fact key={line}>{line}</Fact>
            ))}
          </ul>
          <UsageLine usage={usage} />
          <ModelsUsed models={modelsUsed} />
        </Section>
      </div>
    </article>
  );
}
