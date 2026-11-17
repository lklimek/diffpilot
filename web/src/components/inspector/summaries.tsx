import type {
  Flow,
  NotAnalysedCategory,
  PendingReview,
  PrContextBlock,
  QuietRowRef,
  QuietRows,
  StepId,
  Tour,
} from "../../api/generated/types";
import { ProvenanceBadge } from "../badge/Badge";
import { KindLabel } from "../badge/LlmLabel";
import { PENDING_INCOMPLETE, dependencyFacts, rangeText } from "../section/format";
import { Collapsible } from "./Collapsible";
import type { SectionControl } from "./sectionState";
import { Empty } from "./text";

type Sections = { sections: SectionControl };

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** Summary of a flow introduction: its steps, files, questions and dependencies. */
export function FlowSummary({
  flow,
  flows,
  onSelectStep,
  sections,
}: { flow: Flow; flows: readonly Flow[]; onSelectStep?: (step: StepId) => void } & Sections) {
  const questions = flow.steps.reduce(
    (n, s) => n + s.llm.annotations.filter((a) => a.type === "question").length,
    0,
  );
  const dependencies = flows.filter((f) => flow.llm.depends_on.includes(f.id));
  return (
    <>
      <Collapsible sectionKey="flow-steps" title="Steps of this flow" tone="llm" sections={sections}>
        <ul className="m-0 grid list-none gap-1 p-0">
          {flow.steps.map((s) => {
            const label = `Step ${s.number} · ${s.llm.title}`;
            return (
              <li key={s.id} className="flex flex-wrap items-center gap-1.5">
                {onSelectStep ? (
                  <button
                    type="button"
                    onClick={() => onSelectStep(s.id)}
                    className="min-h-6 cursor-pointer text-left text-accent underline"
                  >
                    {label}
                  </button>
                ) : (
                  <span>{label}</span>
                )}
                <KindLabel kind={s.llm.kind} />
              </li>
            );
          })}
        </ul>
      </Collapsible>
      <Collapsible
        sectionKey="flow-files"
        title="Files touched"
        tone="facts"
        source="From the diff; no LLM."
        sections={sections}
      >
        <ul className="m-0 list-none p-0">
          {flow.facts.files.map((f) => (
            <li key={f}>
              <code>{f}</code>
            </li>
          ))}
        </ul>
      </Collapsible>
      <Collapsible sectionKey="flow-questions" title="Questions to check" tone="llm" sections={sections}>
        <p>
          {questions > 0
            ? `${plural(questions, "question")} across the steps of this flow. Open a step to read them.`
            : "The LLM raised no questions. That is not evidence of no defects."}
        </p>
      </Collapsible>
      <Collapsible sectionKey="flow-depends" title="Depends on" tone="llm" sections={sections}>
        {dependencies.length === 0 ? (
          <Empty>The LLM named no other flow this one depends on.</Empty>
        ) : (
          <ul className="m-0 list-none p-0">
            {dependencies.map((d) => (
              <li key={d.id}>
                Flow {d.position} · {d.llm.title}
              </li>
            ))}
          </ul>
        )}
      </Collapsible>
    </>
  );
}

const CATEGORY: Record<NotAnalysedCategory, string> = {
  tests: "Tests",
  docs_comments: "Docs & comments",
  generated_mechanical: "Mechanical & generated",
};
const RISK_ORDER = [
  ["high", "HIGH"],
  ["medium", "MEDIUM"],
  ["low", "LOW"],
  [null, "not rated"],
] as const;

/**
 * Summary of the PR context. The centre lists CI, counters and profile; the inspector complements it with the
 * coverage of the quiet rows (deterministic) and the LLM's split at a glance.
 */
export function PrContextSummary({
  context,
  quiet,
  flows,
  sections,
}: { context: PrContextBlock; quiet: QuietRows; flows: readonly Flow[] } & Sections) {
  const { facts, llm } = context;
  const unexplained = quiet.unexplained.reduce(
    (n, f) => n + f.ranges.reduce((m, r) => m + r.end_line - r.start_line + 1, 0),
    0,
  );
  const tooLarge = quiet.unexplained
    .filter((f) => f.reason === "too_large_for_prompt")
    .reduce((n, f) => n + f.ranges.length, 0);
  const notAnalysed = quiet.not_analysed.filter((s) => s.file_count > 0);
  const files = notAnalysed.reduce((n, s) => n + s.file_count, 0);
  const risks = RISK_ORDER.map(
    ([risk, label]) => [flows.filter((f) => f.llm.risk === risk).length, label] as const,
  )
    .filter(([n]) => n > 0)
    .map(([n, label]) => `${n} ${label}`);
  return (
    <>
      <Collapsible
        sectionKey="context-coverage"
        title="Coverage"
        tone="facts"
        source="From the diff and the pre-pass; no LLM."
        sections={sections}
      >
        <ul className="m-0 list-disc pl-5">
          <li>
            Unexplained: {plural(unexplained, "changed line")} in {plural(quiet.unexplained.length, "file")}
            {unexplained > 0 && " — read these yourself"}
          </li>
          <li>
            Not analysed: {plural(files, "file")}
            {notAnalysed.length > 0 &&
              ` (${notAnalysed.map((s) => `${CATEGORY[s.category]} ${s.file_count}`).join(", ")})`}
          </li>
          {tooLarge > 0 && <li>{plural(tooLarge, "range")} not sent to the LLM: too large</li>}
          <li>Moves with identical text: {quiet.moves.length}</li>
        </ul>
      </Collapsible>
      <Collapsible sectionKey="context-llm" title="LLM analysis at a glance" tone="llm" sections={sections}>
        <ul className="m-0 list-disc pl-5">
          <li>
            The LLM split this PR into {plural(facts.flow_count, "flow")} with{" "}
            {plural(facts.step_count, "step")}.
          </li>
          {risks.length > 0 && <li>Flows by LLM risk: {risks.join(", ")}</li>}
          <li>
            {facts.related_work_checked
              ? `Related work: ${plural(llm.related_work.length, "item")}`
              : "Related work: not checked (the GitHub App is not installed on this repository)"}
          </li>
          <li>
            {llm.fit_questions.length > 0
              ? `${plural(llm.fit_questions.length, "fit question")} to check`
              : "The LLM raised no fit questions. That is not evidence of no defects."}
          </li>
        </ul>
      </Collapsible>
    </>
  );
}

/** Title of a quiet row. */
export function quietTitle(row: QuietRowRef): string {
  if (row.row === "unexplained") return "Unexplained — read these yourself";
  if (row.row === "moves") return "Moves with identical text";
  return `Not analysed: ${CATEGORY[row.category]}`;
}

/** Summary of a quiet row (deterministic material only). */
export function QuietSummary({
  row,
  quiet,
  context,
  sections,
}: { row: QuietRowRef; quiet: QuietRows; context: PrContextBlock } & Sections) {
  if (row.row === "unexplained") {
    const lines = quiet.unexplained.reduce(
      (n, f) => n + f.ranges.reduce((m, r) => m + r.end_line - r.start_line + 1, 0),
      0,
    );
    return (
      <Collapsible
        sectionKey="quiet"
        title="Unexplained lines"
        tone="facts"
        source="From the diff; no LLM."
        sections={sections}
      >
        <p>
          {plural(lines, "changed line")} that no flow explains. The analysis did not explain them; read them
          yourself. You can comment on any of them.
        </p>
        <ul className="m-0 list-none p-0">
          {quiet.unexplained.map((f) => (
            <li key={`${f.side}:${f.file}`}>
              <code>{f.file}</code>: lines {f.ranges.map(rangeText).join(", ")}
              {f.side === "base" && " (removed lines)"}
            </li>
          ))}
        </ul>
      </Collapsible>
    );
  }
  if (row.row === "moves") {
    return (
      <Collapsible
        sectionKey="quiet"
        title="Moved blocks"
        tone="facts"
        source="Deterministic check; no LLM."
        sections={sections}
      >
        <p>
          Body text identical after whitespace normalisation. Identical text does not mean unchanged
          behaviour.
        </p>
        <ul className="m-0 grid list-none gap-1 p-0">
          {quiet.moves.map((m) => (
            <li key={`${m.to.file}:${m.to.range.start_line}`}>
              <code>
                {m.from.file}:{rangeText(m.from.range)}
              </code>{" "}
              →{" "}
              <code>
                {m.to.file}:{rangeText(m.to.range)}
              </code>{" "}
              <ProvenanceBadge badge={m.badge} />
            </li>
          ))}
        </ul>
      </Collapsible>
    );
  }
  const section = quiet.not_analysed.find((s) => s.category === row.category);
  const counters = context.facts.counters;
  return (
    <Collapsible
      sectionKey="quiet"
      title={CATEGORY[row.category]}
      tone="facts"
      source="Recognised by file path or line rule; no LLM."
      sections={sections}
    >
      {section ? (
        <>
          <p>{plural(section.file_count, "file")}. Shown as a count only; the content was not analysed.</p>
          {row.category === "tests" && (
            <p>{plural(counters.lines_removed_from_tests, "line")} removed from tests.</p>
          )}
          {row.category === "generated_mechanical" &&
            dependencyFacts(counters).map((line) => <p key={line}>{line}.</p>)}
          <ul className="m-0 grid list-none gap-1 p-0">
            {section.items.map((item) => (
              <li key={`${item.side}:${item.file}`}>
                <code>{item.file}</code>
                {item.side === "base" && " (base)"} <ProvenanceBadge badge={item.badge} />{" "}
                <span className="text-muted">{item.rule}</span>
              </li>
            ))}
          </ul>
        </>
      ) : (
        <Empty>No file of this PR is in this section.</Empty>
      )}
    </Collapsible>
  );
}

/** Summary of the user's pending review on GitHub. */
export function PendingSummary({ review, sections }: { review: PendingReview | null } & Sections) {
  let text: string;
  if (review === null) text = "Your pending review is listed in the centre.";
  else if (!review.commenting_available)
    text =
      "Commenting is not available: the smart-review GitHub App is not installed on this repository. Comment on GitHub directly.";
  else
    text = `${plural(review.comments.length, "comment")} in your GitHub pending review.${
      review.incomplete ? ` ${PENDING_INCOMPLETE}` : ""
    } Edit and delete act on GitHub; you submit the review on GitHub.`;
  return (
    <Collapsible
      sectionKey="pending"
      title="Pending review"
      tone="facts"
      source="From GitHub."
      sections={sections}
    >
      <p>{text}</p>
    </Collapsible>
  );
}

/** Title of a flow, e.g. "Flow 1 · Send an RPC request with retry". */
export const flowTitle = (flow: Flow) => `Flow ${flow.position} · ${flow.llm.title}`;

/** The flow of a step, and the step. */
export function findStep(tour: Tour, stepId: StepId) {
  for (const flow of tour.flows) {
    const step = flow.steps.find((s) => s.id === stepId);
    if (step) return { flow, step };
  }
  return undefined;
}
