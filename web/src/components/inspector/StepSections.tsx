import { AskRemarkButton } from "../ask/AskRemarkButton";
import {
  GithubThreadPreview,
  OpenThreadInComments,
  type GithubThreadActions,
} from "../comments/GithubThreadPreview";
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
  Step,
} from "../../api/generated/types";
import { RiskLabel } from "../badge/LlmLabel";
import { threadsNote } from "../section/format";
import { ProvenanceTag } from "../section/ProvenanceTag";
import { Tooltip } from "../tooltip/Tooltip";
import { FlagCard } from "../flag/FlagCard";
import { sortedFlags } from "../flag/flags";
import { Collapsible } from "./Collapsible";
import type { SectionControl } from "./sectionState";
import { BasisNote, Empty, GithubLink } from "./text";

type Sections = { currentFile?: string; sections: SectionControl; onEvidence?: (e: Evidence) => void };
type AskRemark = { onAskRemark?: (annotation: Annotation) => void };

/** Section 1: classic diff of the step or of the selected range (the screen supplies the diff view). */
export function DiffSection({
  diff,
  ofRange,
  title,
  sections,
}: { diff?: ReactNode; ofRange: boolean; title?: string } & Sections) {
  return (
    <Collapsible
      sectionKey="diff"
      title={title ?? (ofRange ? "Classic diff of the selected range" : "Classic diff of this step")}
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
  onAskRemark,
  onEvidence,
  currentFile,
}: { annotations: readonly Annotation[] } & Sections & AskRemark) {
  const behaviour = annotations.filter((a) => a.type === "behaviour" && (a.was?.trim() || a.is?.trim()));
  if (behaviour.length === 0) return null;
  return (
    <Collapsible sectionKey="behaviour" title="Behaviour: was → is" tone="llm" sections={sections}>
      <ul className="m-0 grid list-none gap-2 p-0">
        {behaviour.map((a) => (
          <li key={a.id} data-annotation-id={a.id}>
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
            <BasisNote
              currentFile={currentFile}
              basis={a.basis}
              evidence={a.evidence}
              onEvidence={onEvidence}
            />
            {onAskRemark && <AskRemarkButton onAsk={() => onAskRemark(a)} />}
          </li>
        ))}
      </ul>
    </Collapsible>
  );
}

function TextList({
  annotations,
  onEvidence,
  onAskRemark,
  currentFile,
}: {
  annotations: readonly Annotation[];
  onEvidence?: (e: Evidence) => void;
  currentFile?: string;
} & AskRemark) {
  return (
    <ul className="m-0 list-disc pl-5">
      {annotations.map((a) => (
        <li key={a.id} data-annotation-id={a.id}>
          <CodeText text={a.text ?? ""} />{" "}
          <BasisNote
            currentFile={currentFile}
            basis={a.basis}
            evidence={a.evidence}
            onEvidence={onEvidence}
          />{" "}
          {onAskRemark && <AskRemarkButton onAsk={() => onAskRemark(a)} />}
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
  onAskRemark,
  onEvidence,
  currentFile,
}: { why: string; notes: readonly Annotation[] } & Sections & AskRemark) {
  notes = notes.filter((a) => a.text?.trim());
  if (!why.trim() && notes.length === 0) return null;
  return (
    <Collapsible sectionKey="why" title="Why" tone="llm" sections={sections}>
      <div className="font-semibold">
        <CodeText text={why} />
      </div>
      {notes.length > 0 && (
        <TextList
          onAskRemark={onAskRemark}
          currentFile={currentFile}
          annotations={notes}
          onEvidence={onEvidence}
        />
      )}
    </Collapsible>
  );
}

/** Section 4: consequences. */
export function ConsequencesSection({
  annotations,
  sections,
  onAskRemark,
  onEvidence,
  currentFile,
}: { annotations: readonly Annotation[] } & Sections & AskRemark) {
  annotations = annotations.filter((a) => a.text?.trim());
  if (annotations.length === 0) return null;
  return (
    <Collapsible sectionKey="consequences" title="Consequences" tone="llm" sections={sections}>
      <TextList
        onAskRemark={onAskRemark}
        currentFile={currentFile}
        annotations={annotations}
        onEvidence={onEvidence}
      />
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

/** Section 5: callers and callees from the tech plugin; one factual line while no plugin provides them (v1). */
export function CallHierarchySection({
  hierarchy,
  sections,
}: { hierarchy: CallHierarchy | null } & Sections) {
  if (!hierarchy)
    return (
      <p className="px-3 py-1 text-[12px] text-muted">Call hierarchy: not available for this language yet.</p>
    );
  if (hierarchy.callers.length + hierarchy.callees.length === 0) return null;
  return (
    <Collapsible
      sectionKey="calls"
      title="Call hierarchy"
      tone="facts"
      source="From the language plugin; no LLM."
      sections={sections}
    >
      <Sites title="Callers" sites={hierarchy.callers} />
      <Sites title="Callees" sites={hierarchy.callees} />
    </Collapsible>
  );
}

/** Section 6: the LLM's flags on the step, strongest first; open by default, apart from the Assessment. */
export function FlagsSection({
  annotations,
  sections,
  onAskRemark,
  onEvidence,
  step,
  currentFile,
  onUseAsComment,
}: {
  annotations: readonly Annotation[];
  step: Step;
  /** "Use as review comment" on a flag; no button without it. */
  onUseAsComment?: (flag: Annotation) => void;
} & Sections &
  AskRemark) {
  const flags = sortedFlags(annotations).filter((a) => a.text?.trim());
  if (flags.length === 0) return null;
  return (
    <Collapsible sectionKey="flags" title={`Flags (${flags.length})`} tone="llm" sections={sections}>
      <ul className="m-0 grid list-none gap-2 p-0">
        {flags.map((a) => (
          <FlagCard
            key={a.id}
            annotation={a}
            currentFile={currentFile}
            onEvidence={onEvidence}
            onOpen={
              onEvidence &&
              (() => onEvidence({ file: step.facts.file, side: step.facts.side, line: a.range.start_line }))
            }
            onUseAsComment={onUseAsComment && (() => onUseAsComment(a))}
            onAskRemark={onAskRemark && (() => onAskRemark(a))}
          />
        ))}
      </ul>
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
  if (assessment.risk === "low")
    return (
      <p>
        <RiskLabel risk={assessment.risk} /> <CodeText text={assessment.reason} />
      </p>
    );
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
    ["Origin", assessment.origin ? ORIGIN[assessment.origin] : null],
    ["Merge impact", assessment.blocking ? BLOCKING[assessment.blocking] : null],
    ["Smallest correction", assessment.smallest_correction],
  ];
  return (
    <dl className="m-0">
      {rows
        .filter(([, value]) => value != null && value !== "")
        .map(([term, value]) => (
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
  if (!assessment) return null;
  return (
    <Collapsible sectionKey="assessment" title="Assessment" tone="llm" sections={sections}>
      <AssessmentFields assessment={assessment} />
    </Collapsible>
  );
}

const STATUS: Record<ThreadLlmStatus, string> = {
  still_valid: "still valid",
  stale: "stale",
  already_addressed: "already addressed",
};

function Thread({ thread, actions }: { thread: GithubThread; actions?: GithubThreadActions }) {
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
    <li
      data-thread-id={thread.id}
      className="grid gap-0.5 rounded-md border border-border bg-surface-2 px-2.5 py-1.5"
    >
      <p>
        <strong>@{thread.author}</strong> on{" "}
        <code>
          {thread.path}:{lines}
        </code>{" "}
        <span className="text-muted">{facts.join(", ")}</span>
      </p>
      <GithubThreadPreview thread={thread} actions={actions} />
      {thread.llm_status && (
        <div>
          <ProvenanceTag source="llm" />{" "}
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
      <p className="flex flex-wrap gap-x-3">
        <GithubLink url={thread.url} tip="Open the thread on GitHub (new tab)">
          Open the thread on GitHub
        </GithubLink>
        {actions && <OpenThreadInComments id={thread.id} actions={actions} />}
      </p>
    </li>
  );
}

/** Section 8: GitHub review threads at this location, with the LLM status where the flow analysis gave one. */
export function ThreadsSection({
  threads,
  status,
  sections,
  actions,
}: { threads: readonly GithubThread[]; status: ThreadsStatus; actions?: GithubThreadActions } & Sections) {
  const note = threadsNote(status);
  if (threads.length === 0 && !note) return null;
  return (
    <Collapsible
      sectionKey="threads"
      title="GitHub threads at this location"
      tone="facts"
      tag="github"
      sections={sections}
    >
      {note && <p>{note}.</p>}
      {threads.length > 0 && (
        <ul className="m-0 grid list-none gap-1.5 p-0">
          {threads.map((t) => (
            <Thread key={t.id} thread={t} actions={actions} />
          ))}
        </ul>
      )}
    </Collapsible>
  );
}
