import { focusQuestionHeading } from "./cardKeys";
import { ProposalText } from "./ProposalText";
import { Tooltip } from "../tooltip/Tooltip";
import { Fragment, useEffect, useRef, type ReactNode } from "react";
import type {
  CarryEvent,
  CodeContext,
  CommentItem,
  DecideRequest,
  DecisionBatchRow,
  FlowStepRef,
  Gate,
} from "../../api/generated/types";
import { busyTip, buttonClass } from "../button/button";
import { OpenExternal } from "../dashboard/common/OpenExternal";
import { navigateOnClick, type Navigate } from "../dashboard/common/navigate";
import { clockText, dateTimeText } from "../format/time";
import { ProvenanceTag } from "../section/ProvenanceTag";
import { CollapsibleSection } from "../collapsible/CollapsibleSection";
import { CHOICE_LABEL, decisionLabel, type EffectsPreviewState } from "./decisions";
import type { ReviewComment } from "../../api/provisional/comments";
import { BotHeading, CommentTitle } from "./CommentTitle";
import { DuplicateLinks, DuplicateOfLink } from "./DuplicateLinks";
import { NOT_ASSESSED } from "./queueStatus";
import { DecisionForm } from "./DecisionForm";
import { GateMenu } from "./GateMenu";
import { GithubCommentText } from "./GithubCommentText";
import { NotesElsewhere } from "./NotesElsewhere";
import { CodeText } from "../section/CodeText";
import { Chip, FactChip, GateChip, itemId, judgedAt, sourceText, validityAt } from "./parts";

/** The code context of the card: loading, failed, or the lines. */
export type ContextState =
  | { state: "loading" }
  | { state: "error"; message: string }
  | { state: "none" }
  | { state: "ready"; value: CodeContext };

interface QuestionCardProps {
  item: ReviewComment;
  duplicates?: ReviewComment[];
  primaryHref?: string;
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
  /** The pull request's head now: an assessment of another commit names that commit. */
  prHead: string;
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
  /** The server's plan of the decision drafted on the card (RC16). */
  preview: EffectsPreviewState;
  /** The drafted decision to plan; null while an input is missing. */
  onDraft: (rows: DecisionBatchRow[] | null) => void;
  /** Loads more lines above or below. */
  onMoreLines: (direction: "above" | "below") => void;
  /** "Assess again" (RC6); no button without it. */
  onAssessAgain?: () => void;
  /** The last "Assess again" request is being sent. */
  assessAgainBusy?: boolean;
  /** What the last "Assess again" did, or why it was refused, in plain words. */
  assessAgainNote?: string | null;
  /** Marks or clears a gate (RC15); no "Mark gate" menu without it. */
  onMarkGate?: (gate: Gate, marked: boolean) => void;
  gateBusy?: boolean;
  gateError?: string | null;
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

/**
 * Where GitHub says the comment was written: a round's own review names the round with its commit and end, any
 * other comment its review's commit and its own time; without them the head diffpilot first read it at.
 */
function Raised({ item }: { item: CommentItem }) {
  const written = item.written;
  if (!written)
    return (
      <>
        {" "}
        · first seen at <code>{item.first_seen_head.slice(0, 7)}</code>
      </>
    );
  const on = written.commit && (
    <>
      {" "}
      on <code>{written.commit.slice(0, 7)}</code>,
    </>
  );
  return (
    <>
      {written.round !== undefined ? ` · raised in round ${written.round}` : " · written"}
      {on} {dateTimeText(written.at)}
    </>
  );
}

/** The LLM assessment of the item, or why there is none; `prHead` is the pull request's head now. */
function Assessment({ item, prHead }: { item: CommentItem; prHead: string }) {
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
      {a.evidence_text && (
        <Row term="LLM evidence">
          <CodeText text={a.evidence_text} />
        </Row>
      )}
      <Row term="The LLM says">
        {validityAt(a.validity, a.head, prHead)}
        <span className="block text-xs text-muted">
          Judged on the code at {judgedAt(a.head, prHead)}, not on the code the comment was written on.
        </span>
        {a.evidence.map((e) => (
          <span key={`${e.path}:${e.line}`} className="block text-xs">
            <code>
              {e.path}:{e.line}
            </code>{" "}
            <CodeText text={e.note} />
          </span>
        ))}
      </Row>
      <Row term="Pre-existing on base">
        {a.pre_existing === null ? "not supplied" : a.pre_existing ? "yes" : "no, introduced by this PR"}
      </Row>
      <Row term="Necessity">
        {a.necessity.verdict === "in" ? "in" : "out"}: <CodeText text={a.necessity.reason} />
        {a.necessity.better_pr && ` (better fit: ${a.necessity.better_pr})`}
      </Row>
      <Row term="Gates">{a.gates.length ? a.gates.join(", ") : "none named"}</Row>
      <Row term="Elsewhere">
        {a.elsewhere.length
          ? a.elsewhere.map((e, i) => (
              <Fragment key={`${e.reference}:${i}`}>
                {i > 0 && "; "}
                {e.reference}: <CodeText text={e.note} />
              </Fragment>
            ))
          : "—"}
      </Row>
      <Row term="Fix">
        {a.fix_sketch ? <CodeText text={a.fix_sketch} /> : "—"}
        {a.fix_size && ` · size ${a.fix_size}`}
      </Row>
      <Row term="Previously decided">
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
  useEffect(() => focusQuestionHeading(heading.current), [item.id]);
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
        <Raised item={item} />
      </p>
      <h2
        id={`q-${item.id}`}
        ref={heading}
        tabIndex={-1}
        className="mt-1 mb-1.5 text-lg font-semibold wrap-anywhere"
      >
        {itemId(item)} · <CommentTitle item={item} />
      </h2>
      <BotHeading item={item} />
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
        {item.gone_at && (
          <Chip
            text="Not in the latest read from GitHub"
            tip="The last complete read of the pull request did not show this comment: it was deleted on GitHub or is no longer a comment item (for example a person's earlier review). It is out of the queue; its decisions stay."
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
                    Flow {f.flow_position} · {f.flow_title} · step {f.step_number}
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
        {item.point && (
          <>
            <p>
              Point read by diffpilot's LLM from {item.author}'s{" "}
              {item.point.source_kind === "review_body" ? "review" : "comment"} <ProvenanceTag source="llm" />{" "}
              <OpenExternal href={item.point.source_url} label="Open the point source on GitHub" />
            </p>
            {item.point.source_kind === "review_body" && (
              <FactChip
                text="Quote found in the review"
                tip="The server found this exact quote in the supplied source version."
              />
            )}
          </>
        )}
        <DuplicateLinks primary={item} duplicates={props.duplicates ?? []} />
        {item.duplicate_of != null && (
          <p>
            <DuplicateOfLink item={item} href={props.primaryHref} onNavigate={props.onNavigate} />
          </p>
        )}
        <blockquote
          data-comment-text
          className="m-0 rounded border-l-4 border-border bg-surface-2 px-2.5 py-1.5 whitespace-pre-wrap"
        >
          <GithubCommentText text={item.point?.quote ?? item.body} />
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
        defaultOpen={false}
        storageKey={key("assessment")}
        aside={
          <>
            <ProvenanceTag source="llm" />
            {item.assessment && (
              <span className="text-xs text-muted">
                diffpilot Codex at {item.assessment.head.slice(0, 7)}, {assessedAt}
                {item.assessment.reused_since &&
                  ` · code at these lines unchanged since ${item.assessment.reused_since.slice(0, 7)}`}
                {item.assessment.skills_commit &&
                  ` · Assessed with claudius ${item.assessment.skills_commit.slice(0, 7)}`}
              </span>
            )}
          </>
        }
      >
        <div className="rounded-md border border-llm bg-llm-bg px-3 py-2">
          <Assessment item={item} prHead={props.prHead} />
        </div>
        {rec && (
          <p className="mt-2 mb-0 text-[13px]">
            <b>Recommendation:</b>{" "}
            {item.assessment?.proposed_decision ? (
              <ProposalText proposal={item.assessment.proposed_decision} />
            ) : (
              (CHOICE_LABEL[rec.kind] ?? rec.kind)
            )}{" "}
            — <CodeText text={rec.reason} /> <ProvenanceTag source="llm" />
          </p>
        )}
        {(props.onAssessAgain || props.onMarkGate) && (
          <div className="mt-2 flex flex-wrap items-center gap-2 text-[13px]">
            {props.onAssessAgain && (
              <Tooltip
                tip={busyTip(
                  "Asks diffpilot's LLM to assess this comment again at the current head. Uses LLM tokens.",
                  props.assessAgainBusy ?? false,
                )}
              >
                <button
                  type="button"
                  aria-disabled={props.assessAgainBusy || undefined}
                  onClick={() => !props.assessAgainBusy && props.onAssessAgain?.()}
                  className={buttonClass("plain", true)}
                >
                  Assess again
                </button>
              </Tooltip>
            )}
            {props.onMarkGate && (
              <GateMenu
                gates={item.gates}
                itemLabel={itemId(item)}
                busy={props.gateBusy ?? false}
                error={props.gateError ?? null}
                onMark={props.onMarkGate}
              />
            )}
            {props.assessAgainNote && (
              <span role="status" className="text-muted">
                {props.assessAgainNote}
              </span>
            )}
          </div>
        )}
      </CollapsibleSection>

      <DecisionForm
        key={item.id}
        item={item}
        preview={props.preview}
        onDraft={props.onDraft}
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
