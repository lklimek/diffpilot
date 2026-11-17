import { useId, type ReactNode } from "react";
import type { ImpactRef, PatternRepetition, Step, StepId, ThreadsStatus } from "../../api/generated/types";
import { DifferingLinesBadge, ProvenanceBadge } from "../badge/Badge";
import { KindLabel } from "../badge/LlmLabel";
import { AssessmentFields } from "../inspector/StepSections";
import { GithubLink } from "../inspector/text";
import { rangeText, threadsNote } from "../section/format";
import { LlmLine, Section } from "../section/Section";
import { Tooltip } from "../tooltip/Tooltip";
import { ReviewToggle } from "./ReviewToggle";

interface StepBlockProps {
  /** The step. */
  step: Step;
  /** The code of the step (code view, or its loading / error state). */
  code: ReactNode;
  /** The flow's pattern repetitions; those explained in this step show as "Same pattern × N". */
  patterns?: PatternRepetition[];
  /** Code of an impact reference outside the diff, shown under it; text only without it. */
  impactCode?: (impact: ImpactRef) => ReactNode;
  /** Whether the user ticked this step. */
  reviewed?: boolean;
  /** Ticks (`true`) or unticks (`false`) the step; no button without it. */
  onSetReviewed?: (reviewed: boolean) => void;
  /** Number of the user's pending comments on this step's lines. */
  ownComments?: number;
  /** "Show old code" toggle (the old code itself is drawn in the code slot); no button without it. */
  oldCode?: { shown: boolean; onToggle: () => void };
  /** Opens another step (pattern sites that lie in a step). */
  onSelectStep?: (step: StepId) => void;
  /** Whether the PR's review threads were read completely (`PrContextFacts.threads_status`). */
  threadsStatus?: ThreadsStatus;
}

const HATCH = "bg-[image:repeating-linear-gradient(135deg,transparent_0_6px,rgb(120_130_150/0.16)_6px_7px)]";
const BUTTON =
  "cursor-pointer rounded-md border border-border bg-surface px-2.5 py-1 text-[12.5px] font-semibold whitespace-nowrap text-text hover:border-accent aria-pressed:border-accent aria-pressed:bg-accent-bg";

function Markers({
  step,
  ownComments,
  threadsStatus,
}: {
  step: Step;
  ownComments: number;
  threadsStatus: ThreadsStatus;
}) {
  const note = threadsNote(threadsStatus);
  const remarks = step.llm.annotations.length;
  const threads = step.facts.thread_ids.length;
  const items: [string, string][] = [];
  if (remarks > 0) {
    items.push([
      `◆ ${remarks} LLM ${remarks === 1 ? "remark" : "remarks"}`,
      "LLM annotations in this step; their lines are highlighted in the code. LLM analysis.",
    ]);
  }
  if (threads > 0) {
    const count = `${threads} GitHub ${threads === 1 ? "thread" : "threads"}`;
    items.push([
      threadsStatus === "partial" ? `${count} (the list from GitHub is incomplete)` : count,
      "Existing GitHub review threads on lines of this step (from GitHub); listed in the inspector.",
    ]);
  } else if (note) {
    // Without a complete list, no thread marker must not read as "no threads".
    items.push([note, "GitHub's review threads of this PR were not read completely."]);
  }
  if (ownComments > 0) {
    items.push([
      `▣ ${ownComments} your ${ownComments === 1 ? "comment" : "comments"}`,
      "Your comments in the GitHub pending review on lines of this step.",
    ]);
  }
  if (items.length === 0) return null;
  return (
    <ul
      aria-label="Markers in this step"
      className="m-0 flex list-none flex-wrap gap-2 p-0 text-xs text-muted"
    >
      {items.map(([text, tip]) => (
        <li key={text}>
          <Tooltip tip={tip}>
            <span tabIndex={0} className="cursor-help">
              {text}
            </span>
          </Tooltip>
        </li>
      ))}
    </ul>
  );
}

function CodeSlot({ step, changed, children }: { step: Step; changed: number; children: ReactNode }) {
  if (step.llm.kind !== "context") return <div data-code-slot>{children}</div>;
  return (
    <div
      data-code-slot
      data-context
      className={`rounded-md border-l-[5px] border-dotted border-mv-bd ${HATCH}`}
    >
      {/* With no changed line the code view states that fact itself, once. */}
      <p className="px-2 py-0.5 text-[11px] font-semibold text-muted">
        Context (LLM classification): connects the flow.
        {changed > 0 &&
          ` ${changed} changed ${changed === 1 ? "line" : "lines"} in this range (from the diff).`}
      </p>
      {children}
    </div>
  );
}

function Impact({
  impacts,
  impactCode,
}: {
  impacts: ImpactRef[];
  impactCode?: StepBlockProps["impactCode"];
}) {
  return (
    <Section title="Impact outside the diff" tone="llm">
      <LlmLine />
      <ul className="m-0 grid list-none gap-2.5 p-0 text-[12.5px]">
        {impacts.map((impact) => (
          <li key={`${impact.file}:${impact.range.start_line}`}>
            <p className="flex flex-wrap items-center gap-2">
              <code className="font-semibold">{impact.file}</code>
              {impact.symbol && <span className="text-muted">{impact.symbol}</span>}
              <GithubLink
                url={impact.github_url}
                tip="Open these lines on GitHub at the head commit (new tab)"
              >
                L{rangeText(impact.range)}
              </GithubLink>
            </p>
            <p>
              {impact.reason} <span className="text-muted">({impact.basis})</span>
            </p>
            {impactCode?.(impact)}
          </li>
        ))}
      </ul>
    </Section>
  );
}

function Pattern({
  pattern,
  onSelectStep,
}: {
  pattern: PatternRepetition;
  onSelectStep?: (s: StepId) => void;
}) {
  return (
    <details className="rounded-lg border border-border bg-surface px-4 py-2 text-[12.5px]">
      <summary className="cursor-pointer font-semibold">Same pattern × {pattern.sites.length}</summary>
      <p className="mt-1.5 text-[11.5px] text-llm">LLM analysis unless marked otherwise.</p>
      <p className="mt-1">{pattern.description}</p>
      <ul className="m-0 mt-1 list-disc pl-5">
        {pattern.sites.map((site) => {
          const where = `${site.file}:${rangeText(site.range)}`;
          return (
            <li key={`${where}:${site.side}`}>
              <GithubLink url={site.github_url} tip="Open these lines on GitHub (new tab)">
                {where}
                {site.side === "base" && " (base)"}
              </GithubLink>
              {site.step_id && onSelectStep && (
                <button
                  type="button"
                  aria-label={`Open step for ${where}`}
                  onClick={() => onSelectStep(site.step_id!)}
                  className="ml-2 cursor-pointer text-accent underline"
                >
                  Open step
                </button>
              )}
            </li>
          );
        })}
      </ul>
    </details>
  );
}

/** Step block: header with ticks and markers, facts, the four LLM facts, code slot, collapsed assessment, impact, patterns. */
export function StepBlock({
  step,
  code,
  patterns = [],
  impactCode,
  reviewed = false,
  onSetReviewed,
  ownComments = 0,
  oldCode,
  onSelectStep,
  threadsStatus = "checked",
}: StepBlockProps) {
  const headingId = useId();
  const { facts, llm } = step;
  const changed = facts.changed_lines.reduce((n, r) => n + r.end_line - r.start_line + 1, 0);
  const repeated = patterns.filter((p) => p.explained_in === step.id);
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
        <GithubLink
          url={facts.github_url}
          tip={`Open these lines on GitHub at the ${facts.side} commit (new tab)`}
        >
          L{rangeText(facts.range)}
        </GithubLink>
        <Markers step={step} ownComments={ownComments} threadsStatus={threadsStatus} />
        {(oldCode || onSetReviewed) && (
          <div className="ml-auto flex flex-wrap gap-2">
            {oldCode && (
              <Tooltip tip="Show or hide the classic diff of this step (d)">
                <button
                  type="button"
                  aria-pressed={oldCode.shown}
                  onClick={oldCode.onToggle}
                  className={BUTTON}
                >
                  Show old code
                </button>
              </Tooltip>
            )}
            {onSetReviewed && (
              <ReviewToggle
                label="Mark reviewed"
                tip="Tick this step as reviewed by you (x); a flow is reviewed when all its steps are ticked."
                pressed={reviewed}
                onChange={onSetReviewed}
              />
            )}
          </div>
        )}
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
        <div className="min-w-0 @[1300px]:col-start-2 @[1300px]:row-span-3 @[1300px]:row-start-1">
          <CodeSlot step={step} changed={changed}>
            {code}
          </CodeSlot>
        </div>
        <details className="rounded-lg border border-border bg-surface px-4 py-2 text-[12.5px]">
          <summary className="cursor-pointer font-semibold">Assessment (LLM)</summary>
          <p className="mt-1.5 text-[11.5px] text-llm">LLM analysis unless marked otherwise.</p>
          {llm.assessment ? (
            <AssessmentFields assessment={llm.assessment} />
          ) : (
            <p className="mt-1">The LLM gave no assessment for this step.</p>
          )}
        </details>
        {(llm.impact.length > 0 || repeated.length > 0) && (
          <div className="grid min-w-0 gap-3 @[1300px]:col-span-2">
            {llm.impact.length > 0 && <Impact impacts={llm.impact} impactCode={impactCode} />}
            {repeated.map((p) => (
              <Pattern key={p.description} pattern={p} onSelectStep={onSelectStep} />
            ))}
          </div>
        )}
      </div>
    </article>
  );
}
