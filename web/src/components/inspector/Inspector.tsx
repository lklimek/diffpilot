import { useEffect, useRef, type ReactNode } from "react";
import type {
  Annotation,
  AnnotationId,
  AnnotationType,
  BlockRef,
  Evidence,
  GithubThread,
  PendingReview,
  Step,
  StepId,
  Tour,
} from "../../api/generated/types";
import { DifferingLinesBadge, ProvenanceBadge } from "../badge/Badge";
import { useFlashTarget } from "../flash/useFlashTarget";
import { GroupLabel, KindLabel, RiskLabel } from "../badge/LlmLabel";
import { FlowWorkLabel } from "../flow-intro/status";
import { annotationName, rangeText } from "../section/format";
import { EvidenceLinks } from "../section/EvidenceLinks";
import { Collapsible } from "./Collapsible";
import { useSectionState, type SectionControl } from "./sectionState";
import { Tooltip } from "../tooltip/Tooltip";
import {
  AssessmentSection,
  BehaviourSection,
  CallHierarchySection,
  ConsequencesSection,
  DiffSection,
  FlagsSection,
  ThreadsSection,
  WhySection,
} from "./StepSections";
import {
  FlowSummary,
  PendingSummary,
  PrContextSummary,
  QuietSummary,
  findStep,
  flowTitle,
  quietTitle,
} from "./summaries";
import { stepGithubLink } from "../github/links";
import { GithubLink } from "./text";

/** Props of the inspector panel. */
export interface InspectorProps {
  /** The analysis. */
  tour: Tour;
  /** The block shown in the centre; the inspector describes it. */
  block: BlockRef;
  /** The highlight the user clicked (an annotation of the current step); null = describe the block. */
  annotationId?: AnnotationId | null;
  /** Classic diff of the step or of the selected annotation's range (section 1), e.g. code-view `ClassicDiff`. */
  diff?: ReactNode;
  /** Dock under the sections: feedback and comment (e.g. `FeedbackButtons`, `CommentComposer`). */
  dock?: ReactNode;
  /** The Ask chat's questions about this place (a read-only copy), after the panel's sections. */
  questions?: ReactNode;
  /** The user's pending review, for the pending-review summary. */
  pendingReview?: PendingReview | null;
  /** Leaves the selected annotation ("Back to step"). */
  onBackToStep?: () => void;
  /** Opens a step (from the flow summary). */
  onSelectStep?: (step: StepId) => void;
  onEvidence?: (evidence: Evidence) => void;
  /** Scroll to and briefly highlight a remark or GitHub thread of the step ("Show in Inspector"); new `nonce`, again. */
  flash?: { kind: "annotation" | "thread"; id: string; nonce: number } | null;
  /** The `flash` request was handled (its highlight ended); it is not to be shown again. */
  onFlashed?: () => void;
  /** Section state shared with the screen; the panel keeps its own without it. */
  sections?: SectionControl;
  /** "Use as review comment" on a flag of the step; no button without it. */
  onFlagAsComment?: (flag: Annotation) => void;
  /** What to show instead of the block's own panel (the Files view's file, hunk or selected lines). */
  panel?: InspectorPanel;
  /** A way back above the heading ("← Back to hunk"), e.g. from a step narrowed in the Files view. */
  back?: InspectorBack;
}

/** A way back to a wider subject. */
export interface InspectorBack {
  label: string;
  tip: string;
  /** What it goes back to, beside the link. */
  context?: string;
  onBack: () => void;
}

// The section listing each annotation type.
const SECTION_OF: Record<AnnotationType, string> = {
  behaviour: "behaviour",
  note: "why",
  consequence: "consequences",
  flag: "flags",
};

const linesWord = (r: Annotation["range"]) =>
  `${r.start_line === r.end_line ? "line" : "lines"} ${rangeText(r)}`;

const overlaps = (t: GithubThread, step: Step, a: Annotation) =>
  t.path === step.facts.file &&
  t.side === step.facts.side &&
  t.line !== null &&
  (t.start_line ?? t.line) <= a.range.end_line &&
  t.line >= a.range.start_line;

/** One Inspector panel: heading, its labels, provenance and sections. */
export interface InspectorPanel {
  title: string;
  /** Under the title: labels and links. */
  meta?: ReactNode;
  /** Where deterministic content comes from; LLM panels carry tags on their sections instead. */
  provenance?: string;
  body: ReactNode;
  /** Set when an annotation is selected: context line for Back to step. */
  backTo?: string;
}
type Panel = InspectorPanel;

function stepPanel(
  tour: Tour,
  step: Step,
  annotation: Annotation | undefined,
  diff: ReactNode,
  sections: SectionControl,
  onEvidence?: (evidence: Evidence) => void,
  onFlagAsComment?: (flag: Annotation) => void,
): Panel {
  const stepTitle = `Step ${step.number} · ${step.llm.title}`;
  const github = stepGithubLink(tour.analysis.pr, step.facts);
  return {
    title: annotation ? `${annotationName(annotation)} · ${linesWord(annotation.range)}` : stepTitle,
    backTo: annotation ? stepTitle : undefined,
    meta: (
      <>
        <KindLabel kind={step.llm.kind} />
        <GithubLink url={github.url} tip={github.tip}>
          {step.facts.file}:{rangeText(step.facts.range)}
        </GithubLink>
      </>
    ),
    body: (
      <StepDetails
        tour={tour}
        step={step}
        annotation={annotation}
        diff={diff}
        sections={sections}
        onEvidence={onEvidence}
        onFlagAsComment={onFlagAsComment}
      />
    ),
  };
}

/** What a step's sections show: the whole step or one remark; the Files view leaves out the Flows-only parts. */
export interface StepDetailsProps {
  tour: Tour;
  step: Step;
  annotation?: Annotation;
  /** The classic diff section; none in the Files view, whose panel places its own diff. */
  diff?: ReactNode;
  sections: SectionControl;
  onEvidence?: (evidence: Evidence) => void;
  onFlagAsComment?: (flag: Annotation) => void;
  /** The Files view: no Called by / Goes next, no diff, no threads (its panel shows those of the lines). */
  files?: boolean;
}

/** A step's sections in Flows order: behaviour, why, consequences, call hierarchy, flags, assessment, threads. */
export function StepDetails({
  tour,
  step,
  annotation,
  diff,
  sections,
  onEvidence,
  onFlagAsComment,
  files = false,
}: StepDetailsProps) {
  const anns = annotation ? [annotation] : step.llm.annotations;
  const byType = (type: AnnotationType) => anns.filter((a) => a.type === type);
  const stepThreads = tour.threads.filter((t) => step.facts.thread_ids.includes(t.id));
  return (
    <>
      {step.facts.moved && (
        <Collapsible
          sectionKey="moved"
          title="Moved code"
          tone="facts"
          source="From the diff; no LLM."
          sections={sections}
        >
          <p>Moved from the base commit.</p>
          {step.facts.moved.badge ? (
            <ProvenanceBadge badge={step.facts.moved.badge} />
          ) : (
            <DifferingLinesBadge count={step.facts.moved.differing_lines} />
          )}
          <EvidenceLinks
            currentFile={step.facts.file}
            onEvidence={onEvidence}
            evidence={[
              ...new Set([step.facts.moved.from.range.start_line, step.facts.moved.from.range.end_line]),
            ].map((line) => ({ file: step.facts.moved!.from.file, side: "base", line }))}
          />
        </Collapsible>
      )}
      {[
        ["Called by", step.llm.called_by],
        ["Goes next", step.llm.next],
      ]
        .filter(([, text]) => !files && text?.trim())
        .map(([title, text]) => (
          <Collapsible key={title} sectionKey={title!} title={title!} tone="llm" sections={sections}>
            <p>{text}</p>
          </Collapsible>
        ))}
      {!files && <DiffSection diff={diff} ofRange={annotation !== undefined} sections={sections} />}
      <BehaviourSection
        currentFile={step.facts.file}
        onEvidence={onEvidence}
        annotations={annotation ? anns : byType("behaviour")}
        sections={sections}
      />
      <WhySection
        currentFile={step.facts.file}
        onEvidence={onEvidence}
        why={step.llm.why}
        notes={byType("note")}
        sections={sections}
      />
      <ConsequencesSection
        currentFile={step.facts.file}
        onEvidence={onEvidence}
        annotations={byType("consequence")}
        sections={sections}
      />
      <CallHierarchySection hierarchy={step.facts.call_hierarchy} sections={sections} />
      <FlagsSection
        currentFile={step.facts.file}
        step={step}
        onEvidence={onEvidence}
        annotations={byType("flag")}
        sections={sections}
        onUseAsComment={onFlagAsComment}
      />
      <AssessmentSection assessment={step.llm.assessment} sections={sections} />
      {!files && (
        <ThreadsSection
          threads={annotation ? stepThreads.filter((t) => overlaps(t, step, annotation)) : stepThreads}
          status={tour.pr_context.facts.threads_status}
          sections={sections}
        />
      )}
    </>
  );
}

function panelFor(props: InspectorProps, sections: SectionControl): Panel {
  const { tour, block, annotationId, diff, pendingReview, onSelectStep } = props;
  const deterministic = "Deterministic: from the diff and the built-in rules; no LLM.";
  switch (block.block) {
    case "step": {
      const found = findStep(tour, block.step_id);
      if (!found) break;
      const annotation = found.step.llm.annotations.find((a) => a.id === annotationId);
      return stepPanel(tour, found.step, annotation, diff, sections, props.onEvidence, props.onFlagAsComment);
    }
    case "flow_intro": {
      const flow = tour.flows.find((f) => f.id === block.flow_id);
      if (!flow) break;
      return {
        title: flowTitle(flow),
        meta: (
          <>
            {flow.analysis_status && flow.analysis_status.state !== "analysed" ? (
              <FlowWorkLabel status={flow.analysis_status} analysis={tour.analysis.state} />
            ) : (
              <RiskLabel risk={flow.llm.risk} />
            )}
            <GroupLabel group={flow.group} />
          </>
        ),
        body: (
          <FlowSummary
            onEvidence={props.onEvidence}
            flow={flow}
            flows={tour.flows}
            onSelectStep={onSelectStep}
            sections={sections}
          />
        ),
      };
    }
    case "pr_context":
      return {
        title: "PR context",
        meta: (
          <span className="text-muted">
            #{tour.analysis.pr.number} {tour.analysis.pr.title}
          </span>
        ),
        body: (
          <PrContextSummary
            context={tour.pr_context}
            quiet={tour.quiet}
            flows={tour.flows}
            sections={sections}
          />
        ),
      };
    case "quiet":
      return {
        title: quietTitle(block.row),
        provenance: deterministic,
        body: (
          <QuietSummary row={block.row} quiet={tour.quiet} context={tour.pr_context} sections={sections} />
        ),
      };
    case "pending_review":
      return {
        title: "Pending review",
        provenance: "From your GitHub pending review; no LLM.",
        body: <PendingSummary review={pendingReview ?? null} sections={sections} />,
      };
  }
  return {
    title: "Inspector",
    provenance: deterministic,
    body: <p className="px-3 py-2">This block is not part of the analysis.</p>,
  };
}

/** Right-hand inspector (brief §4.3): never empty; the current block, or the selected annotation of a step. */
export function Inspector(props: InspectorProps) {
  const { dock, onBackToStep, flash, onFlashed } = props;
  const own = useSectionState(JSON.stringify(props.block));
  const sections = props.sections ?? own;
  const panel = props.panel ?? panelFor(props, sections);
  const heading = useRef<HTMLHeadingElement>(null);
  const focusHeading = useRef(false);
  const scroller = useRef<HTMLDivElement>(null);
  const root = useRef<HTMLDivElement>(null);
  const opened = useRef<number | null>(null);
  const attribute = flash?.kind === "thread" ? "data-thread-id" : "data-annotation-id";
  useFlashTarget(
    scroller,
    flash && { selector: `[${attribute}="${flash.id.replace(/["\\]/g, "\\$&")}"]`, nonce: flash.nonce },
    onFlashed,
  );

  // A flashed item in a collapsed section: open that section once per request.
  useEffect(() => {
    if (!flash || opened.current === flash.nonce) return;
    opened.current = flash.nonce;
    const step = props.block.block === "step" ? findStep(props.tour, props.block.step_id)?.step : undefined;
    const type = step?.llm.annotations.find((a) => a.id === flash.id)?.type;
    const key = flash.kind === "thread" ? "threads" : type && SECTION_OF[type];
    if (key && !sections.isOpen(key)) sections.reveal(key);
  });

  const back: InspectorBack | undefined =
    props.back ??
    (panel.backTo && onBackToStep
      ? {
          label: "← Back to step",
          tip: "Shows the whole step again: all its LLM remarks, not only this one.",
          context: panel.backTo,
          onBack: onBackToStep,
        }
      : undefined);
  const backKey = back ? back.label + (back.context ?? "") : null;
  const lastBack = useRef(backKey);
  // After going back the button is gone or changed; the heading takes the focus.
  useEffect(() => {
    if (lastBack.current === backKey) return;
    lastBack.current = backKey;
    if (focusHeading.current) {
      focusHeading.current = false;
      heading.current?.focus();
    }
  });

  return (
    <div ref={root} data-inspector className="flex h-full min-h-0 flex-col bg-surface">
      <div ref={scroller} tabIndex={0} className="min-h-0 flex-1 overflow-auto overscroll-contain">
        <div className="px-3 pt-2.5 pb-1.5">
          {back && (
            <p className="mb-1 flex flex-wrap items-baseline gap-2 text-[12px]">
              <Tooltip tip={back.tip}>
                <button
                  type="button"
                  onClick={() => {
                    focusHeading.current = true;
                    back.onBack();
                  }}
                  className="cursor-pointer text-accent underline"
                >
                  {back.label}
                </button>
              </Tooltip>
              {back.context && <span className="text-muted">{back.context}</span>}
            </p>
          )}
          <h2 ref={heading} tabIndex={-1} className="m-0 text-[15px] font-bold">
            {panel.title}
          </h2>
          {panel.meta && <p className="mt-1 flex flex-wrap items-center gap-2">{panel.meta}</p>}
          {panel.provenance && <p className="mt-1 text-[11.5px] text-muted">{panel.provenance}</p>}
          <p className="sr-only" aria-live="polite">
            {panel.title}
          </p>
        </div>
        {panel.body}
        {props.questions}
      </div>
      {dock && (
        <div
          role="group"
          aria-label="Feedback and comment"
          data-dock
          className="flex max-h-[42vh] shrink-0 flex-col gap-2.5 overflow-auto border-t-2 border-border bg-surface-2 px-3 py-2.5 [&>*]:shrink-0"
        >
          {dock}
        </div>
      )}
    </div>
  );
}
