import { CodeText } from "../section/CodeText";
import type { ReactNode } from "react";
import type {
  Annotation,
  Evidence,
  Assessment,
  CallHierarchy,
  CallSite,
  GithubThread,
  ThreadsStatus,
  ThreadLlmStatus,
} from "../../api/generated/types";
import { RiskLabel } from "../badge/LlmLabel";
import { rangeText, threadsNote } from "../section/format";
import { Tooltip } from "../tooltip/Tooltip";
import { Collapsible } from "./Collapsible";
import type { SectionControl } from "./sectionState";
import { BasisNote, Empty, GithubLink } from "./text";

type Sections = { sections: SectionControl; onEvidence?: (e: Evidence) => void };

const NO_QUESTIONS = "The LLM raised no questions. That is not evidence of no defects.";

/** Section 1: classic diff of the step or of the selected range (the screen supplies the diff view). */
export function DiffSection({ diff, ofRange, sections }: { diff?: ReactNode; ofRange: boolean } & Sections) {
  return (
    <Collapsible
      sectionKey="diff"
      title={ofRange ? "Classic diff of the selected range" : "Classic diff of this step"}
      tone="facts"
      source="From the diff; no LLM."
      sections={sections}
    >
      {diff ?? <Empty>The classic diff is not loaded.</Empty>}
    </Collapsible>
  );
}

/** Section 2: behaviour was → is of the annotations. */
export function BehaviourSection({
  annotations,
  sections,
  onEvidence,
}: { annotations: readonly Annotation[] } & Sections) {
  const behaviour = annotations.filter((a) => a.type === "behaviour");
  return (
    <Collapsible sectionKey="behaviour" title="Behaviour: was → is" tone="llm" sections={sections}>
      {behaviour.length === 0 ? (
        <Empty>The LLM described no was → is change here.</Empty>
      ) : (
        <ul className="m-0 grid list-none gap-2 p-0">
          {behaviour.map((a) => (
            <li key={a.id}>
              <p className="text-[11px] text-muted">
                {a.range.start_line === a.range.end_line ? "Line" : "Lines"} {rangeText(a.range)}{" "}
                <BasisNote basis={a.basis} evidence={a.evidence} onEvidence={onEvidence} />
              </p>
              <dl className="m-0 grid grid-cols-[auto_1fr] gap-x-2">
                <dt className="font-bold text-muted">Was</dt>
                <dd className="m-0">
                  <CodeText text={a.was} />
                </dd>
                <dt className="font-bold text-muted">Is</dt>
                <dd className="m-0">
                  <CodeText text={a.is} />
                </dd>
              </dl>
            </li>
          ))}
        </ul>
      )}
    </Collapsible>
  );
}

function TextList({
  annotations,
  onEvidence,
}: {
  annotations: readonly Annotation[];
  onEvidence?: (e: Evidence) => void;
}) {
  return (
    <ul className="m-0 list-disc pl-5">
      {annotations.map((a) => (
        <li key={a.id}>
          <CodeText text={a.text ?? ""} />{" "}
          <BasisNote basis={a.basis} evidence={a.evidence} onEvidence={onEvidence} />
        </li>
      ))}
    </ul>
  );
}

/** Section 3: why the step is needed, plus the LLM's notes on it. */
export function WhySection({
  why,
  notes,
  sections,
  onEvidence,
}: { why: string; notes: readonly Annotation[] } & Sections) {
  return (
    <Collapsible sectionKey="why" title="Why" tone="llm" sections={sections}>
      <div className="font-semibold">
        <CodeText text={why} />
      </div>
      {notes.length > 0 && <TextList annotations={notes} onEvidence={onEvidence} />}
    </Collapsible>
  );
}

/** Section 4: consequences. */
export function ConsequencesSection({
  annotations,
  sections,
  onEvidence,
}: { annotations: readonly Annotation[] } & Sections) {
  return (
    <Collapsible sectionKey="consequences" title="Consequences" tone="llm" sections={sections}>
      {annotations.length === 0 ? (
        <Empty>The LLM named no consequences here. That is not evidence that there are none.</Empty>
      ) : (
        <TextList annotations={annotations} onEvidence={onEvidence} />
      )}
    </Collapsible>
  );
}

function Sites({ title, sites }: { title: string; sites: readonly CallSite[] }) {
  if (sites.length === 0) return null;
  return (
    <>
      <h4 className="mt-1 text-[11px] font-bold text-muted uppercase">{title}</h4>
      <ul className="m-0 list-none p-0">
        {sites.map((s) => (
          <li key={`${s.file}:${s.line}:${s.symbol}`}>
            <GithubLink url={s.github_url} tip="Open this line on GitHub (new tab)">
              {s.symbol}
            </GithubLink>{" "}
            <span className="text-muted">
              {s.file}:{s.line}
            </span>
          </li>
        ))}
      </ul>
    </>
  );
}

/** Section 5: callers and callees from the tech plugin; n/a in v1 (brief §3). */
export function CallHierarchySection({
  hierarchy,
  sections,
}: { hierarchy: CallHierarchy | null } & Sections) {
  return (
    <Collapsible
      sectionKey="calls"
      title="Call hierarchy"
      tone="facts"
      source="From the language plugin; no LLM."
      sections={sections}
    >
      {hierarchy === null ? (
        <Tooltip tip="Callers and callees come from a language plugin (static analysis). The built-in generic plugin has none, so they were not checked.">
          <span tabIndex={0} className="cursor-help text-muted">
            n/a — needs a language plugin
          </span>
        </Tooltip>
      ) : (
        <>
          <Sites title="Callers" sites={hierarchy.callers} />
          <Sites title="Callees" sites={hierarchy.callees} />
          {hierarchy.callers.length + hierarchy.callees.length === 0 && (
            <Empty>The language plugin found no callers or callees.</Empty>
          )}
        </>
      )}
    </Collapsible>
  );
}

/** Section 6: LLM suspicions phrased as questions to check. */
export function QuestionsSection({
  questions,
  sections,
  onEvidence,
}: { questions: readonly Annotation[] } & Sections) {
  const title = questions.length > 0 ? `Questions to check (${questions.length})` : "Questions to check";
  return (
    <Collapsible sectionKey="questions" title={title} tone="llm" sections={sections}>
      {questions.length === 0 ? (
        <p>{NO_QUESTIONS}</p>
      ) : (
        <TextList annotations={questions} onEvidence={onEvidence} />
      )}
    </Collapsible>
  );
}

const ORIGIN = {
  introduced: "introduced by this PR",
  pre_existing: "pre-existing",
  unclear: "unclear",
} as const;
const BLOCKING = {
  no_action_proposed: "no action proposed",
  blocks_this_pr: "blocks this PR",
  follow_up: "follow-up",
} as const;

/** The assessment fields of brief §3 (shared by the inspector and the step block). */
export function AssessmentFields({ assessment }: { assessment: Assessment }) {
  const rows: [string, ReactNode][] = [
    ["What a production user or operator would notice", assessment.headline],
    [
      "Risk at system scale",
      <>
        <RiskLabel risk={assessment.risk} /> <CodeText text={assessment.reason} />
      </>,
    ],
    ["Basis", assessment.basis_note],
    ["Not checked", assessment.not_checked],
    ["Origin", ORIGIN[assessment.origin]],
    ["Merge impact", BLOCKING[assessment.blocking]],
    ["Smallest correction", assessment.smallest_correction ?? "none given"],
  ];
  return (
    <dl className="m-0">
      {rows.map(([term, value]) => (
        <div key={term}>
          <dt className="mt-1.5 text-[10.5px] font-bold tracking-wide text-muted uppercase">{term}</dt>
          <dd className="m-0">{typeof value === "string" ? <CodeText text={value} /> : value}</dd>
        </div>
      ))}
    </dl>
  );
}

/** Section 7: the step assessment, collapsed by default (verdict after evidence). */
export function AssessmentSection({ assessment, sections }: { assessment: Assessment | null } & Sections) {
  return (
    <Collapsible sectionKey="assessment" title="Assessment" tone="llm" sections={sections}>
      {assessment ? (
        <AssessmentFields assessment={assessment} />
      ) : (
        <Empty>The LLM gave no assessment for this step.</Empty>
      )}
    </Collapsible>
  );
}

const STATUS: Record<ThreadLlmStatus, string> = {
  still_valid: "still valid",
  stale: "stale",
  already_addressed: "already addressed",
};

function Thread({ thread }: { thread: GithubThread }) {
  const lines =
    thread.line === null
      ? "file"
      : thread.start_line !== null
        ? `${thread.start_line}–${thread.line}`
        : `${thread.line}`;
  const facts = [
    `${thread.comment_count} ${thread.comment_count === 1 ? "comment" : "comments"}`,
    thread.is_resolved && "resolved",
    thread.is_outdated && "outdated",
  ].filter(Boolean);
  return (
    <li className="grid gap-0.5 rounded-md border border-border bg-surface-2 px-2.5 py-1.5">
      <p>
        <strong>@{thread.author}</strong> on{" "}
        <code>
          {thread.path}:{lines}
        </code>{" "}
        <span className="text-muted">{facts.join(", ")}</span>
      </p>
      <p className="whitespace-pre-wrap">{thread.excerpt}</p>
      {thread.llm_status && (
        <div>
          <Tooltip tip="LLM status from the analysis of this flow: whether the thread still applies to the final code. LLM analysis, not a GitHub state.">
            <span
              data-llm-label
              tabIndex={0}
              className="cursor-help rounded bg-llm-bg px-1.5 text-[11px] font-semibold text-llm"
            >
              {STATUS[thread.llm_status]}
            </span>
          </Tooltip>{" "}
          <CodeText text={thread.llm_reason ?? ""} />
        </div>
      )}
      <p>
        <GithubLink url={thread.url} tip="Open the thread on GitHub (new tab)">
          Open the thread on GitHub
        </GithubLink>
      </p>
    </li>
  );
}

/** Section 8: GitHub review threads at this location, with the LLM status where the flow analysis gave one. */
export function ThreadsSection({
  threads,
  status,
  sections,
}: { threads: readonly GithubThread[]; status: ThreadsStatus } & Sections) {
  const note = threadsNote(status);
  return (
    <Collapsible
      sectionKey="threads"
      title="GitHub threads at this location"
      tone="facts"
      source="From GitHub; a status label is LLM analysis."
      sections={sections}
    >
      {note && <p>{note}.</p>}
      {threads.length === 0 ? (
        !note && <Empty>No GitHub review threads on these lines.</Empty>
      ) : (
        <ul className="m-0 grid list-none gap-1.5 p-0">
          {threads.map((t) => (
            <Thread key={t.id} thread={t} />
          ))}
        </ul>
      )}
    </Collapsible>
  );
}
