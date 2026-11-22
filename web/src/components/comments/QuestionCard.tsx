import { Tooltip } from "../tooltip/Tooltip";
import { useEffect, useRef, type ReactNode } from "react";
import type {
  CarryEvent,
  CodeContext,
  CommentItem,
  DecideRequest,
  FlowStepRef,
} from "../../api/generated/types";
import { buttonClass } from "../button/button";
import { OpenExternal } from "../dashboard/common/OpenExternal";
import { navigateOnClick, type Navigate } from "../dashboard/common/navigate";
import { clockText } from "../format/time";
import { ProvenanceTag } from "../section/ProvenanceTag";
import { CollapsibleSection } from "../collapsible/CollapsibleSection";
import { CHOICE_LABEL, decisionLabel } from "./decisions";
import { DecisionForm } from "./DecisionForm";
import { NotesElsewhere } from "./NotesElsewhere";
import { Chip, FactChip, GateChip, GithubLinkedText, itemId, sourceText, VALIDITY } from "./parts";

/** The code context of the card: loading, failed, or the lines. */
export type ContextState =
  | { state: "loading" }
  | { state: "error"; message: string }
  | { state: "none" }
  | { state: "ready"; value: CodeContext };

const NOT_ASSESSED = {
  daily_limit: "daily limit",
  failed: "the assessment failed",
  missing: "left out of the pass",
};

interface QuestionCardProps {
  item: CommentItem;
  /** The carry-over event that asked this repeat again (US9), when it is one. */
  reask?: CarryEvent;
  /** "Question 2 of 9". */
  position: { index: number; total: number };
  /** PR number and its one-sentence purpose (LLM, or the analysis's PR context). */
  prNumber: number;
  purpose: string | null;
  context: ContextState;
  /** In-app link of a flow step (opened in a new tab). */
  flowHref: (ref: FlowStepRef) => string;
  /** "Open in Files" target when an analysis of this head exists. */
  filesHref: string | null;
  /** The PR page ("No analysis yet · Start on the PR page"). */
  prPageHref: string;
  onNavigate: Navigate;
  /** Answers in the Ask chat about this item. */
  askAnswers: number;
  /** Opens the shared Ask chat with this item as subject; absent when Ask is not available. */
  onAsk?: () => void;
  askUnavailable?: string;
  /** Opens the chat at this item's threads. */
  onOpenAnswers?: () => void;
  onDecide: (request: DecideRequest) => void;
  onSkip: () => void;
  /** Loads more lines above or below. */
  onMoreLines: (direction: "above" | "below") => void;
  busy: boolean;
  error: string | null;
  now: Date;
  /** Where its sections' states are remembered (view and user). */
  storageKey: string;
}

/** Lines of the context with the commented ones marked. */
function Code({ context, onMore }: { context: CodeContext; onMore: (d: "above" | "below") => void }) {
  const first = context.lines[0]?.line ?? 1;
  const last = context.lines.at(-1)?.line ?? 0;
  const marked = context.marked;
  const isMarked = (n: number) => marked !== null && n >= marked.start_line && n <= marked.end_line;
  return (
    <>
      {first > 1 && (
        <Tooltip tip="Loads more lines above the shown range.">
          <button
            type="button"
            onClick={() => onMore("above")}
            className={`${buttonClass("plain", true)} mb-1`}
          >
            Show more above
          </button>
        </Tooltip>
      )}
      <pre
        aria-label={`Code of ${context.path} at ${context.head.slice(0, 7)}, the commented lines marked`}
        tabIndex={0}
        className="m-0 overflow-auto rounded-md border border-border bg-code-bg py-1.5 font-mono text-[12.5px] leading-[1.55]"
      >
        {context.lines.map((l) => (
          <span
            key={l.line}
            data-marked={isMarked(l.line) || undefined}
            className={`block pr-2.5 whitespace-pre ${isMarked(l.line) ? "bg-med-bg shadow-[inset_4px_0_0_var(--color-med-bd)]" : ""}`}
          >
            <i className="inline-block w-11 pr-2.5 text-right text-muted not-italic select-none">{l.line}</i>
            {l.text}
            {isMarked(l.line) && <span className="sr-only"> (commented line)</span>}
          </span>
        ))}
      </pre>
      {last < context.total_lines && (
        <Tooltip tip="Loads more lines below the shown range.">
          <button
            type="button"
            onClick={() => onMore("below")}
            className={`${buttonClass("plain", true)} mt-1`}
          >
            Show more below
          </button>
        </Tooltip>
      )}
    </>
  );
}

const Row = ({ term, children }: { term: string; children: ReactNode }) => (
  <>
    <dt className="text-muted">{term}</dt>
    <dd className="m-0">{children}</dd>
  </>
);

/** The LLM assessment of the item, or why there is none. */
function Assessment({ item }: { item: CommentItem }) {
  const a = item.assessment;
  if (item.assessment_state.state === "running")
    return <p className="m-0">LLM assessment running for this comment.</p>;
  if (item.assessment_state.state === "not_assessed")
    return (
      <p className="m-0">
        Not assessed: {NOT_ASSESSED[item.assessment_state.reason] ?? item.assessment_state.reason}. Decide
        without a recommendation, or ask for it again later.
      </p>
    );
  if (!a) return <p className="m-0">No assessment stored.</p>;
  const earlier = item.decisions.slice(0, -1);
  return (
    <dl className="m-0 grid grid-cols-[max-content_1fr] gap-x-3 gap-y-1 text-[13px]">
      <Row term="The LLM says">
        {VALIDITY[a.validity] ?? a.validity}
        {a.evidence.map((e) => (
          <span key={`${e.path}:${e.line}`} className="block text-xs">
            <code>
              {e.path}:{e.line}
            </code>{" "}
            {e.note}
          </span>
        ))}
      </Row>
      <Row term="Pre-existing on base">{a.pre_existing ? "yes" : "no, introduced by this PR"}</Row>
      <Row term="Necessity">
        {a.necessity.verdict === "in" ? "in" : "out"}: {a.necessity.reason}
        {a.necessity.better_pr && ` (better fit: ${a.necessity.better_pr})`}
      </Row>
      <Row term="Gates">{a.gates.length ? a.gates.join(", ") : "none named"}</Row>
      <Row term="Elsewhere">
        {a.elsewhere.length ? a.elsewhere.map((e) => `${e.reference}: ${e.note}`).join("; ") : "—"}
      </Row>
      <Row term="Fix">
        {a.fix_sketch ?? "—"}
        {a.fix_size && ` · size ${a.fix_size}`}
      </Row>
      <Row term="Earlier decisions">
        {earlier.length
          ? earlier.map((d) => decisionLabel(d)).join(", ")
          : item.repeat_of
            ? `see C${item.repeat_of}`
            : "none on this item"}
      </Row>
    </dl>
  );
}

/** "Why you are asked again": the rule and the evidence diffpilot read (US9). */
function Reask({ event, now, storageKey }: { event: CarryEvent; now: Date; storageKey: string }) {
  const lines = event.evidence.lines;
  return (
    <CollapsibleSection title="Why you are asked again" storageKey={storageKey}>
      <div className="rounded-md border border-med-bd bg-med-bg px-3 py-2 text-[13px]">
        <p className="m-0 mb-1 font-semibold">
          {event.reason === "bot_edited"
            ? "The bot edited its comment text after your decision."
            : "The code at that place changed after your decision."}
        </p>
        <dl className="m-0 grid grid-cols-[max-content_1fr] gap-x-3 gap-y-0.5">
          <Row term="Your decision">
            C{event.source_item_no} · {CHOICE_LABEL[event.source_kind] ?? event.source_kind}
          </Row>
          {event.evidence.commit && (
            <Row term="Changed lines">
              {lines ? `${lines.start_line}-${lines.end_line} ` : ""}changed in{" "}
              <code>{event.evidence.commit.slice(0, 7)}</code>
            </Row>
          )}
          <Row term="Seen">{clockText(event.at, now)}</Row>
          <Row term="Rule">
            a repeat is asked again when the bot edited its comment text or the code at that place changed
          </Row>
        </dl>
        <p className="mt-1 mb-0">
          <FactChip
            text="read by diffpilot"
            tip="Computed from the comment text hashes or the diff between the two heads."
          />{" "}
          <OpenExternal href={event.evidence.url} label={`Open C${event.item_no} on GitHub`} />
        </p>
      </div>
    </CollapsibleSection>
  );
}

/** One question (plan §3.2): context, evidence, LLM assessment, recommendation, then the decision. */
export function QuestionCard(props: QuestionCardProps) {
  const { item, context, now, storageKey } = props;
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => heading.current?.focus(), [item.id]);
  const ctx = context.state === "ready" ? context.value : null;
  const assessedAt = item.assessment ? clockText(item.assessment.assessed_at, now) : null;
  const rec = item.assessment?.recommendation;
  const key = (section: string) => `${storageKey}:${section}`;
  return (
    <article
      aria-labelledby={`q-${item.id}`}
      className="rounded-lg border border-border bg-surface px-4 py-3.5"
    >
      <p className="m-0 text-xs text-muted">
        {itemId(item)} · {props.position.index} of {props.position.total} · {sourceText(item)}
        {item.first_round !== null && ` · first seen round ${item.first_round}`} at{" "}
        <code>{item.first_seen_head.slice(0, 7)}</code>
      </p>
      <h2 id={`q-${item.id}`} ref={heading} tabIndex={-1} className="mt-1 mb-1.5 text-lg font-semibold">
        {itemId(item)} · {item.title}
      </h2>
      <div className="flex flex-wrap items-center gap-1.5">
        {item.gates.map((g) => (
          <GateChip key={g.gate} gate={g.gate} source={g.source} />
        ))}
        {item.state === "reopened" && item.repeat_of !== null && (
          <Chip
            text={`Asked again — repeat of C${item.repeat_of}`}
            tip="A repeat of a decided finding, asked again because the bot edited its text or the code at that place changed."
            tone="reask"
          />
        )}
        {item.assessment && (
          <Chip text={`Category: ${item.assessment.category}`} tip="Category from the LLM assessment." />
        )}
        {item.bot_severity && (
          <span className="inline-flex items-center gap-1 text-xs">
            <ProvenanceTag source="github" /> {item.author}: {item.bot_severity}
          </span>
        )}
        <OpenExternal href={item.url} label={`Open ${itemId(item)} on GitHub`} />
        {props.askAnswers > 0 && (
          <Tooltip tip="Opens the Ask chat at the answers about this comment.">
            <button
              type="button"
              onClick={props.onOpenAnswers}
              className="inline-flex min-h-6 cursor-pointer items-center rounded-full border border-accent bg-accent-bg px-2 text-xs text-accent"
            >
              {props.askAnswers} Ask {props.askAnswers === 1 ? "answer" : "answers"}
            </button>
          </Tooltip>
        )}
      </div>
      <div className="mt-2 flex flex-wrap items-center gap-2 text-xs">
        <span className="text-muted">Notes about this code elsewhere:</span>
        {ctx ? <NotesElsewhere notes={ctx.notes} /> : <span className="text-muted">—</span>}
      </div>

      {props.reask && <Reask event={props.reask} now={now} storageKey={key("reask")} />}

      <CollapsibleSection
        title="What the PR is about"
        storageKey={key("purpose")}
        aside={<ProvenanceTag source="llm" />}
      >
        <p className="m-0">
          <b>PR #{props.prNumber}:</b> {props.purpose ?? "No purpose stated yet."}
        </p>
      </CollapsibleSection>

      <CollapsibleSection title="In flows" storageKey={key("flows")}>
        {!ctx || ctx.analysis === null ? (
          <p className="m-0 text-[13px] text-muted">
            No analysis yet.{" "}
            <a href={props.prPageHref} onClick={navigateOnClick(props.prPageHref, props.onNavigate)}>
              Start on the PR page
            </a>
          </p>
        ) : ctx.in_flows.length === 0 ? (
          <p className="m-0 text-[13px] text-muted">No flow step of the analysis contains these lines.</p>
        ) : (
          <>
            <ul className="m-0 grid gap-0.5 pl-5 text-[13px]">
              {ctx.in_flows.map((f) => (
                <li key={`${f.flow}-${f.step}`}>
                  <a href={props.flowHref(f)} target="_blank" rel="noopener noreferrer">
                    {f.flow_title} › step {f.step}
                    <span className="sr-only"> (opens in a new tab)</span>
                  </a>
                </li>
              ))}
            </ul>
            {ctx.analysis_head && (
              <p className="mt-0.5 text-xs text-muted">
                From the analysis at {ctx.analysis_head.slice(0, 7)}.
              </p>
            )}
          </>
        )}
      </CollapsibleSection>

      <CollapsibleSection
        title="Comment"
        storageKey={key("comment")}
        aside={<ProvenanceTag source="github" />}
      >
        <blockquote
          data-comment-text
          className="m-0 rounded border-l-4 border-border bg-surface-2 px-2.5 py-1.5 whitespace-pre-wrap"
        >
          <GithubLinkedText text={item.body} />
        </blockquote>
      </CollapsibleSection>

      {item.location && (
        <CollapsibleSection title="The discussed code" storageKey={key("code")}>
          {context.state === "loading" && <p className="m-0 text-muted">Loading the code…</p>}
          {context.state === "error" && (
            <p role="alert" className="m-0 text-high">
              {context.message}
            </p>
          )}
          {ctx && (
            <>
              <p className="m-0 mb-1 text-xs text-muted">
                <code>{ctx.path}</code> at <code>{ctx.head.slice(0, 7)}</code>
                {props.filesHref && (
                  <>
                    {" · "}
                    <a href={props.filesHref} target="_blank" rel="noopener noreferrer">
                      Open in Files<span className="sr-only"> (opens in a new tab)</span>
                    </a>
                  </>
                )}
              </p>
              <Code context={ctx} onMore={props.onMoreLines} />
              {ctx.code && (
                <div className="mt-1.5 rounded-md border border-llm bg-llm-bg px-3 py-2 text-[13px]">
                  <b>What this code does</b> <ProvenanceTag source="llm" />
                  <p className="m-0">{ctx.code.description}</p>
                </div>
              )}
            </>
          )}
        </CollapsibleSection>
      )}

      <CollapsibleSection title="Facts read by diffpilot" storageKey={key("facts")}>
        <div className="flex flex-wrap gap-1.5">
          {item.github.resolved !== null && (
            <FactChip
              text={
                item.github.resolved
                  ? `Thread resolved${item.github.resolved_by ? ` by ${item.github.resolved_by}` : ""}`
                  : "Thread unresolved"
              }
              tip="Read from GitHub."
            />
          )}
          {item.github.outdated && (
            <FactChip text="Outdated" tip="GitHub marks the thread outdated: its lines changed." />
          )}
          {item.github.edited_after_decision && (
            <FactChip
              text="Edited after the decision"
              tip="The comment text changed after the latest decision (text hash)."
            />
          )}
          {item.github.marker && (
            <FactChip
              text={`Marker ${item.github.marker}`}
              tip="Parsed from the comment body; used to recognise repeats."
            />
          )}
        </div>
      </CollapsibleSection>

      <CollapsibleSection
        title="LLM assessment"
        storageKey={key("assessment")}
        aside={
          <>
            <ProvenanceTag source="llm" />
            {item.assessment && (
              <span className="text-xs text-muted">
                diffpilot Codex at {item.assessment.head.slice(0, 7)}, {assessedAt}
                {item.assessment.reused_since &&
                  ` · code at these lines unchanged since ${item.assessment.reused_since.slice(0, 7)}`}
              </span>
            )}
          </>
        }
      >
        <div className="rounded-md border border-llm bg-llm-bg px-3 py-2">
          <Assessment item={item} />
        </div>
        {rec && (
          <p className="mt-2 mb-0 text-[13px]">
            <b>Recommendation:</b> {CHOICE_LABEL[rec.kind] ?? rec.kind} — {rec.reason}{" "}
            <ProvenanceTag source="llm" />
          </p>
        )}
      </CollapsibleSection>

      <DecisionForm
        key={`${item.id}:${item.revision}`}
        item={item}
        onDecide={props.onDecide}
        onAsk={props.onAsk}
        askUnavailable={props.askUnavailable}
        onSkip={props.onSkip}
        busy={props.busy}
        error={props.error}
        storageKey={storageKey}
      />
    </article>
  );
}
