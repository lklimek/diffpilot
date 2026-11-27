import { CodeText } from "../section/CodeText";
import { useId, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import type {
  FileViewedState,
  ImpactRef,
  PatternRepetition,
  PatternSite,
  Step,
  StepId,
  ThreadsStatus,
} from "../../api/generated/types";
import { KindLabel } from "../badge/LlmLabel";
import { siteGithubLink, stepGithubLink, type GithubLineLink, type PrLinkBase } from "../github/links";
import { GithubLink } from "../inspector/text";
import { rangeText, threadsNote } from "../section/format";
import { ProvenanceTag } from "../section/ProvenanceTag";
import { Section } from "../section/Section";
import { AskButton } from "../ask/chat/AskButton";
import { Tooltip } from "../tooltip/Tooltip";
import { ReviewToggle } from "./ReviewToggle";
import { ViewedChip } from "../files/ViewedChip";

interface StepBlockProps {
  /** The step. */
  step: Step;
  /** The pull request; with it the header links the step's changed lines in the PR diff on GitHub. */
  pr?: PrLinkBase;
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
  /** Looks up a step of the tour; with `pr`, pattern sites in a step's changed lines link the PR diff. */
  stepOf?: (step: StepId) => Step | undefined;
  /** Whether the PR's review threads were read completely (`PrContextFacts.threads_status`). */
  threadsStatus?: ThreadsStatus;
  /** The file's GitHub Viewed mark (null: not read); no chip while unknown (undefined). */
  viewed?: FileViewedState | null;
  /** Opens the Files view at this step; the file path stays plain text without it. */
  onOpenFile?: () => void;
  /** Opens the Ask chat about this step; no button without it. */
  onAsk?: () => void;
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
  if (step.llm.kind !== "context")
    return (
      <div data-code-slot className="min-w-0">
        {children}
      </div>
    );
  return (
    <div
      data-code-slot
      data-context
      className={`min-w-0 rounded-md border-l-[5px] border-dotted border-mv-bd ${HATCH}`}
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

/** One line of an impact reason, with a disclosure when more text is available. */
function ImpactReason({ reason, basis }: Pick<ImpactRef, "reason" | "basis">) {
  const text = reason.trim();
  const firstLine = text.split(/\r?\n/, 1)[0] ?? "";
  const hasDetails = firstLine !== text;
  const [clipped, setClipped] = useState(false);
  const [expanded, setExpanded] = useState(false);
  const excerpt = useRef<HTMLSpanElement>(null);
  const detailsId = useId();
  const disclose = hasDetails || clipped || expanded;

  useLayoutEffect(() => {
    const element = excerpt.current;
    if (!element || hasDetails) return;
    const measure = () => setClipped(element.scrollWidth > element.clientWidth);
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [basis, firstLine, hasDetails]);

  const preview = (linkPaths: boolean) => (
    <>
      <CodeText text={firstLine} linkPaths={linkPaths} />
      {hasDetails ? "… " : " "}
      <span className="text-muted">({basis})</span>
    </>
  );
  return (
    <div className="relative min-w-0">
      {/* Measure the linked line even when the clipped button needs noninteractive text. */}
      {!hasDetails && (
        <span
          ref={excerpt}
          inert={disclose || undefined}
          aria-hidden={disclose || undefined}
          className={`block truncate ${disclose ? "invisible absolute inset-x-0 top-0" : ""}`}
        >
          {preview(true)}
        </span>
      )}
      {disclose && (
        <Tooltip
          tip={expanded ? "Collapses this explanation to one line." : "Shows the full impact explanation."}
          className="block"
        >
          <button
            type="button"
            className="block min-h-6 w-full cursor-pointer rounded text-left text-accent underline decoration-dotted underline-offset-2"
            aria-controls={expanded ? detailsId : undefined}
            aria-expanded={expanded}
            onClick={() => setExpanded(!expanded)}
          >
            <span className="block truncate">{preview(false)}</span>
          </button>
        </Tooltip>
      )}
      {expanded && (
        <div id={detailsId} className="mt-1 whitespace-pre-line">
          <CodeText text={reason} /> <span className="text-muted">({basis})</span>
        </div>
      )}
    </div>
  );
}

/** One impact: file header, its explanation, and its code collapsed until the header is clicked. */
function ImpactItem({
  impact,
  impactCode,
}: {
  impact: ImpactRef;
  impactCode?: StepBlockProps["impactCode"];
}) {
  const [open, setOpen] = useState(false);
  const where = `${impact.file} lines ${rangeText(impact.range)}`;
  return (
    <li className="min-w-0">
      <p className="flex flex-wrap items-center gap-2">
        {impactCode ? (
          <Tooltip tip="Show or hide the code of these lines.">
            <button
              type="button"
              aria-expanded={open}
              aria-label={where}
              onClick={() => setOpen(!open)}
              className={`min-h-6 cursor-pointer rounded font-mono font-semibold before:mr-1 before:inline-block before:w-3 before:text-muted ${open ? "before:content-['▾']" : "before:content-['▸']"}`}
            >
              {impact.file}
            </button>
          </Tooltip>
        ) : (
          <code className="font-semibold">{impact.file}</code>
        )}
        {impact.symbol && <span className="text-muted">{impact.symbol}</span>}
        <GithubLink url={impact.github_url} tip="Open these lines on GitHub at the head commit (new tab)">
          L{rangeText(impact.range)}
        </GithubLink>
      </p>
      <ImpactReason reason={impact.reason} basis={impact.basis} />
      {open && impactCode?.(impact)}
    </li>
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
      <ul className="m-0 grid list-none gap-2.5 p-0 text-[12.5px]">
        {impacts.map((impact) => (
          <ImpactItem
            key={`${impact.file}:${impact.range.start_line}`}
            impact={impact}
            impactCode={impactCode}
          />
        ))}
      </ul>
    </Section>
  );
}

function Pattern({
  pattern,
  onSelectStep,
  linkOf,
}: {
  pattern: PatternRepetition;
  onSelectStep?: (s: StepId) => void;
  linkOf: (site: PatternSite) => GithubLineLink;
}) {
  return (
    <details className="rounded-lg border border-border bg-surface px-4 py-2 text-[12.5px]">
      <summary className="cursor-pointer font-semibold">Same pattern × {pattern.sites.length}</summary>
      <div className="mt-1.5">
        <ProvenanceTag source="llm" /> <CodeText text={pattern.description} />
      </div>
      <ul className="m-0 mt-1 list-disc pl-5">
        {pattern.sites.map((site) => {
          const where = `${site.file}:${rangeText(site.range)}`;
          const link = linkOf(site);
          return (
            <li key={`${where}:${site.side}`}>
              <GithubLink url={link.url} tip={link.tip}>
                {where}
                {site.side === "base" && " (base)"}
              </GithubLink>
              {site.step_id && onSelectStep && (
                <Tooltip tip="Goes to the step of this tour that shows these lines." className="ml-2">
                  <button
                    type="button"
                    aria-label={`Open step for ${where}`}
                    onClick={() => onSelectStep(site.step_id!)}
                    className="cursor-pointer text-accent underline"
                  >
                    Open step
                  </button>
                </Tooltip>
              )}
            </li>
          );
        })}
      </ul>
    </details>
  );
}

/** Step title, decision, review actions and full-width code; details live in the Inspector. */
export function StepBlock({
  step,
  pr,
  code,
  patterns = [],
  impactCode,
  reviewed = false,
  onSetReviewed,
  ownComments = 0,
  oldCode,
  onSelectStep,
  stepOf,
  threadsStatus = "checked",
  viewed,
  onOpenFile,
  onAsk,
}: StepBlockProps) {
  const headingId = useId();
  const { facts, llm } = step;
  const changed = facts.changed_lines.reduce((n, r) => n + r.end_line - r.start_line + 1, 0);
  const repeated = patterns.filter((p) => p.explained_in === step.id);
  const github = stepGithubLink(pr, facts);
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
        {onOpenFile ? (
          <Tooltip tip="Opens this file in the Files view at this step's lines (s).">
            <button
              type="button"
              onClick={onOpenFile}
              className="cursor-pointer border-0 bg-transparent p-0 font-mono font-semibold text-accent underline"
            >
              {facts.file}
            </button>
          </Tooltip>
        ) : (
          <code className="font-semibold">{facts.file}</code>
        )}
        {viewed !== undefined && <ViewedChip state={viewed} path={facts.file} />}
        {llm.symbol && <span className="text-muted">{llm.symbol}</span>}
        <GithubLink url={github.url} tip={github.tip}>
          L{rangeText(facts.range)}
        </GithubLink>
        <Markers step={step} ownComments={ownComments} threadsStatus={threadsStatus} />
        <div className="flex w-full min-w-0 items-start gap-2">
          {llm.decides.trim() && (
            <div data-decides className="min-w-0 flex-1 text-sm">
              <CodeText text={llm.decides} /> <ProvenanceTag source="llm" />
            </div>
          )}
          {(oldCode || onSetReviewed || onAsk) && (
            <div className="ml-auto flex shrink-0 flex-wrap gap-2">
              {onAsk && <AskButton what="this step" onAsk={onAsk} />}
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
        </div>
      </header>
      <div className="grid min-w-0 content-start gap-3 p-3">
        <CodeSlot step={step} changed={changed}>
          {code}
        </CodeSlot>
        {(llm.impact.length > 0 || repeated.length > 0) && (
          <div className="grid min-w-0 gap-3">
            {llm.impact.length > 0 && <Impact key={step.id} impacts={llm.impact} impactCode={impactCode} />}
            {repeated.map((p) => (
              <Pattern
                key={p.description}
                pattern={p}
                onSelectStep={onSelectStep}
                linkOf={(site) =>
                  siteGithubLink(pr, site, site.step_id ? stepOf?.(site.step_id)?.facts : undefined)
                }
              />
            ))}
          </div>
        )}
      </div>
    </article>
  );
}
