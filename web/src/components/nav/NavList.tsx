import { useId, type ReactNode } from "react";
import type {
  AnalysisState,
  AnnotationId,
  BlockRef,
  Flow,
  QuietRowRef,
  QuietRows,
  Step,
  StepId,
} from "../../api/generated/types";
import { GroupLabel, RiskLabel } from "../badge/LlmLabel";
import { FlagMarker } from "../flag/FlagMarker";
import { isFlowReviewed } from "../flow-intro/progress";
import { FlowWorkLabel } from "../flow-intro/status";
import { NOT_ANALYSED_LABEL, fileCount, lineCount, plural } from "../quiet/labels";
import { RemarkPill } from "../remark-pills/RemarkPills";
import { summariseRemarks } from "../remark-pills/remarks";
import { Tooltip } from "../tooltip/Tooltip";
import { Kbd } from "../help/Kbd";

interface NavListProps {
  /** Flows in tour order. */
  flows: Flow[];
  /** State of the whole analysis: pending flows say what they wait for; without it, only the flow's own state. */
  analysis?: AnalysisState;
  /** Quiet rows of the analysis, listed after every flow. */
  quiet: QuietRows;
  /** Steps the user ticked. */
  ticked: StepId[];
  /** Comments in the pending review; `"unavailable"` without the GitHub App; no row when omitted. */
  pendingCount?: number | `${number}+` | "unavailable" | "loading";
  /** The block shown in the centre. */
  current: BlockRef;
  /** Called with the block the user picks. */
  onSelect: (block: BlockRef) => void;
  /** Number of the user's own questions per step; steps without one are omitted. */
  questionCounts?: Readonly<Record<StepId, number>>;
  /** The entry of the user's questions page; no entry when omitted. */
  questions?: { count: number | "loading" | "error"; open: boolean; onOpen: () => void };
  /** Remarks with the user's feedback (their pills dim). */
  handledRemarks?: ReadonlySet<AnnotationId>;
  /** A rating/remark pill: go to the first remark among `steps`; without it no pills are shown. */
  onRemark?: (steps: Step[]) => void;
  /** A flag marker: go to the first flag at the strongest level among `steps`; without it no markers are shown. */
  onFlag?: (steps: Step[]) => void;
  /** Ticks (`true`) or unticks (`false`) every step of a flow; no flow toggle without it. */
  onSetFlowReviewed?: (flow: Flow, reviewed: boolean) => void;
}

// Spoken count of the questions entry while it has no number.
const QUESTIONS_COUNT: Record<string, string> = { loading: "loading", error: "could not be read" };

const ITEM =
  "block w-full cursor-pointer rounded-md border-l-4 border-transparent px-2 py-1.5 text-left text-text hover:bg-surface-2 aria-[current]:border-accent aria-[current]:bg-accent-bg aria-[current]:font-semibold";
const QUIET_ITEM = `${ITEM} text-[12.5px] text-mv`;
const SMALL = "block text-[11px] font-normal text-muted";
const FLOW_TICK =
  "inline-flex size-6 shrink-0 cursor-pointer items-center justify-center rounded border-0 bg-transparent font-bold text-muted hover:bg-surface-2 aria-pressed:text-accent";

const currentFlowId = (flows: Flow[], current: BlockRef): string | undefined => {
  if (current.block === "flow_intro") return current.flow_id;
  if (current.block === "step") return flows.find((f) => f.steps.some((s) => s.id === current.step_id))?.id;
  return undefined;
};

const ariaCurrent = (isCurrent: boolean) => (isCurrent ? ("step" as const) : undefined);

const sameRow = (current: BlockRef | undefined, row: QuietRowRef) =>
  current?.block === "quiet" && JSON.stringify(current.row) === JSON.stringify(row);

function Heading({ children }: { children: ReactNode }) {
  return <h3 className="mt-3 mb-1 text-[10.5px] font-bold tracking-wide text-muted uppercase">{children}</h3>;
}

interface QuietRowProps {
  row: QuietRowRef;
  tip: string;
  current: BlockRef | undefined;
  onSelect: (block: BlockRef) => void;
  children: ReactNode;
}

function QuietRow({ row, tip, current, onSelect, children }: QuietRowProps) {
  return (
    <li>
      <Tooltip tip={tip} className="block" placement="right">
        <button
          type="button"
          className={QUIET_ITEM}
          aria-current={sameRow(current, row) || undefined}
          onClick={() => onSelect({ block: "quiet", row })}
        >
          {children}
        </button>
      </Tooltip>
    </li>
  );
}

function pendingText(count: NonNullable<NavListProps["pendingCount"]>): string {
  if (count === "unavailable") return "not available";
  if (count === "loading") return "loading…";
  // "N+": GitHub has more pending comments than were read.
  return typeof count === "string" ? `${count} comments` : plural(count, "comment");
}

/** Left menu: PR context, flows in tour order with risk and ticks, quiet rows, pending review. */
export function NavList({
  flows,
  analysis,
  quiet,
  ticked,
  pendingCount,
  current: shown,
  onSelect,
  questionCounts = {},
  questions,
  handledRemarks = new Set(),
  onRemark,
  onFlag,
  onSetFlowReviewed,
}: NavListProps) {
  const openFlow = currentFlowId(flows, shown);
  // The questions page replaces the block in the centre: no block row is current meanwhile.
  const current = questions?.open ? undefined : shown;
  const unexplainedLines = quiet.unexplained.reduce((n, f) => n + lineCount(f.ranges), 0);
  const sections = quiet.not_analysed.filter((s) => s.items.length > 0);
  const rowProps = { current, onSelect };
  const contextHint = useId();
  return (
    <div className="text-sm">
      <button
        type="button"
        data-row="menu"
        className={`${ITEM} mb-1 font-semibold`}
        aria-current={ariaCurrent(current?.block === "pr_context")}
        aria-label="PR context"
        aria-describedby={contextHint}
        onClick={() => onSelect({ block: "pr_context" })}
      >
        PR context
        <span id={contextHint} className={SMALL}>
          Problem, related work, fit questions, CI
        </span>
      </button>
      <Heading>Flows</Heading>
      <ol className="m-0 list-none p-0">
        {flows.map((flow) => {
          const reviewed = isFlowReviewed(flow, ticked);
          return (
            <li key={flow.id} className="mb-1">
              <div className="flex items-start gap-1">
                <button
                  type="button"
                  data-row="menu"
                  className={`${ITEM} min-w-0 flex-1 break-words`}
                  aria-current={ariaCurrent(current?.block === "flow_intro" && current.flow_id === flow.id)}
                  onClick={() => onSelect({ block: "flow_intro", flow_id: flow.id })}
                  aria-label={`Flow ${flow.position}: ${flow.llm.title}${reviewed ? ", reviewed" : ""}`}
                >
                  {reviewed && !onSetFlowReviewed && (
                    <span aria-hidden className="float-right font-bold text-accent">
                      ☑
                    </span>
                  )}
                  <b>{flow.position}</b> {flow.llm.title}
                </button>
                <span className="flex flex-col items-end gap-0.5 pt-1.5">
                  {flow.analysis_status && flow.analysis_status.state !== "analysed" ? (
                    <FlowWorkLabel status={flow.analysis_status} analysis={analysis} />
                  ) : (
                    <RiskLabel risk={flow.llm.risk} />
                  )}
                  <GroupLabel group={flow.group} />
                </span>
              </div>
              {/* Under the title, so the title keeps the row's width: tick toggle, flag marker, remark pill. */}
              <div className="flex flex-wrap items-center gap-1 pl-3 empty:hidden">
                {onSetFlowReviewed && flow.steps.length > 0 && (
                  <Tooltip
                    tip={
                      reviewed
                        ? `Unticks all ${plural(flow.steps.length, "step")} of flow ${flow.position}.`
                        : `Ticks all ${plural(flow.steps.length, "step")} of flow ${flow.position} as reviewed by you; press again to untick them.`
                    }
                  >
                    <button
                      type="button"
                      aria-pressed={reviewed}
                      aria-label={`Mark flow ${flow.position} reviewed`}
                      className={FLOW_TICK}
                      onClick={() => onSetFlowReviewed(flow, !reviewed)}
                    >
                      <span aria-hidden>{reviewed ? "☑" : "☐"}</span>
                    </button>
                  </Tooltip>
                )}
                {onFlag && (
                  <FlagMarker
                    annotations={flow.steps.flatMap((s) => s.llm.annotations)}
                    scope="flow"
                    subject={`flow ${flow.position}`}
                    onSelect={() => onFlag(flow.steps)}
                  />
                )}
                {onRemark && (
                  <RemarkPill
                    summary={summariseRemarks(flow.steps, handledRemarks)}
                    scope="flow"
                    subject={`flow ${flow.position}`}
                    withRating={false}
                    onSelect={() => onRemark(flow.steps)}
                  />
                )}
              </div>
              {flow.id === openFlow && (
                <ol className="my-0.5 mb-1.5 ml-3.5 list-none border-l-2 border-border py-0 pl-1.5">
                  {flow.steps.map((step) => {
                    const done = ticked.includes(step.id);
                    const asked = questionCounts[step.id] ?? 0;
                    return (
                      <li key={step.id} className="flex items-start gap-1">
                        <button
                          type="button"
                          className={`${ITEM} flex-1 py-0.5 text-[12.5px]`}
                          aria-current={ariaCurrent(current?.block === "step" && current.step_id === step.id)}
                          onClick={() => onSelect({ block: "step", step_id: step.id })}
                          aria-label={`Step ${step.number}: ${step.llm.title}${asked > 0 ? `, ${plural(asked, "question")} asked` : ""}${done ? ", reviewed" : ""}`}
                        >
                          <span aria-hidden className="inline-block w-3.5 font-bold text-accent">
                            {done ? "☑" : ""}
                          </span>
                          <Tooltip
                            tip={`Step kind: ${step.llm.kind === "context" ? "unchanged context" : step.llm.kind === "added" ? "added" : `changed (${step.llm.kind.replaceAll("_", " ")})`}. LLM classification; the diff marks changed lines.`}
                          >
                            <span
                              tabIndex={0}
                              aria-label={`Step kind: ${step.llm.kind}`}
                              className="mr-1 font-mono"
                            >
                              {step.llm.kind === "context" ? "=" : step.llm.kind === "added" ? "+" : "~"}
                            </span>
                          </Tooltip>
                          <b>{step.number}</b> {step.llm.title}
                          {asked > 0 && (
                            <Tooltip tip={`${plural(asked, "question")} asked on this step`}>
                              <span
                                tabIndex={0}
                                className="ml-1 rounded-full bg-accent-bg px-1 font-mono text-[11px] font-semibold text-accent"
                              >
                                ?{asked}
                              </span>
                            </Tooltip>
                          )}
                        </button>
                        <span className="flex flex-col items-end gap-0.5 pt-0.5">
                          {onRemark && (
                            <RemarkPill
                              summary={summariseRemarks([step], handledRemarks)}
                              scope="step"
                              subject={`step ${step.number}`}
                              onSelect={() => onRemark([step])}
                            />
                          )}
                          {onFlag && (
                            <FlagMarker
                              annotations={step.llm.annotations}
                              scope="step"
                              subject={`step ${step.number}`}
                              onSelect={() => onFlag([step])}
                            />
                          )}
                        </span>
                      </li>
                    );
                  })}
                </ol>
              )}
            </li>
          );
        })}
      </ol>
      {(quiet.unexplained.length > 0 || quiet.moves.length > 0) && (
        <>
          <Heading>Quiet rows</Heading>
          <ul aria-label="Quiet rows" className="m-0 list-none p-0">
            {quiet.unexplained.length > 0 && (
              <QuietRow
                row={{ row: "unexplained" }}
                tip="Changed lines the analysis did not assign to any flow. Read them yourself."
                {...rowProps}
              >
                <strong>Unexplained — read these yourself</strong>
                <small className={SMALL}>
                  {plural(unexplainedLines, "changed line")} in {plural(fileCount(quiet.unexplained), "file")}
                </small>
              </QuietRow>
            )}
            {quiet.moves.length > 0 && (
              <QuietRow
                row={{ row: "moves" }}
                tip="Moved blocks whose text is identical to the original after whitespace normalisation (deterministic check)."
                {...rowProps}
              >
                Moves with identical text
                <small className={SMALL}>{plural(quiet.moves.length, "moved block")}</small>
              </QuietRow>
            )}
          </ul>
        </>
      )}
      {sections.length > 0 && (
        <>
          <Heading>Not analysed</Heading>
          <ul aria-label="Not analysed" className="m-0 list-none p-0">
            {sections.map((s) => (
              <QuietRow
                key={s.category}
                row={{ row: "not_analysed", category: s.category }}
                tip="Recognised by a built-in path or line rule; the LLM did not analyse it."
                {...rowProps}
              >
                {NOT_ANALYSED_LABEL[s.category]} · {plural(s.file_count, "file")}
              </QuietRow>
            ))}
          </ul>
        </>
      )}
      {pendingCount !== undefined && (
        <>
          <Heading>Pending review</Heading>
          <Tooltip
            tip="Your pending review on GitHub: the comments you added, not yet submitted."
            className="block"
            placement="right"
          >
            <button
              type="button"
              data-row="menu"
              className={ITEM}
              aria-current={current?.block === "pending_review" || undefined}
              aria-label={`Pending review on GitHub: ${pendingText(pendingCount)}`}
              onClick={() => onSelect({ block: "pending_review" })}
            >
              Pending review on GitHub{" "}
              <b className="ml-1 rounded-full bg-surface-2 px-1.5 text-xs whitespace-nowrap">
                {pendingText(pendingCount)}
              </b>
            </button>
          </Tooltip>
        </>
      )}
      {questions && (
        <>
          <Heading>Your questions</Heading>
          <Tooltip
            tip={
              questions.count === "error"
                ? "Your questions could not be read; the page says why. Only you see them."
                : "Every question you asked in this review, with its state and the LLM answer. Only you see them."
            }
            className="block"
            placement="right"
          >
            <button
              type="button"
              data-row="menu"
              className={ITEM}
              aria-current={questions.open ? "page" : undefined}
              aria-label={`Your questions and answers: ${QUESTIONS_COUNT[questions.count] ?? questions.count}`}
              onClick={questions.onOpen}
            >
              Questions and answers{" "}
              <b className="ml-1 rounded-full bg-surface-2 px-1.5 text-xs whitespace-nowrap">
                {questions.count === "loading" ? "…" : questions.count === "error" ? "!" : questions.count}
              </b>
            </button>
          </Tooltip>
        </>
      )}
      <p className="mt-3 text-xs text-muted">
        <Kbd>j</Kbd> <Kbd>k</Kbd> next / previous block · <Kbd>n</Kbd> <Kbd>p</Kbd> flow · <Kbd>?</Kbd> help
      </p>
    </div>
  );
}
