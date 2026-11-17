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
import { rangeText } from "../section/format";
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

const SOURCE = { repository_file: "repository file", memcan: "MemCan" } as const;

/** Summary of the PR context: deterministic facts apart from the LLM analysis. */
export function PrContextSummary({ context, sections }: { context: PrContextBlock } & Sections) {
  const { facts, llm } = context;
  const sources = facts.project_profile.sources;
  const deps = facts.counters.dependencies_changed;
  return (
    <>
      <Collapsible
        sectionKey="context-facts"
        title="Facts"
        tone="facts"
        source="From the diff, the pre-pass and the repository; no LLM."
        sections={sections}
      >
        <ul className="m-0 list-disc pl-5">
          <li>
            {plural(facts.flow_count, "flow")}, {plural(facts.step_count, "step")}
          </li>
          <li>{plural(facts.counters.lines_removed_from_tests, "line")} removed from tests</li>
          <li>
            {plural(deps.length, "dependency", "dependencies")} changed
            {deps.length > 0 && `: ${deps.join(", ")}`}
          </li>
          <li>
            {sources.length > 0
              ? `Project profile: ${sources.map((s) => SOURCE[s]).join(" and ")}`
              : "No project profile — ratings are component-level"}
          </li>
          {facts.agent_instruction_files.length > 0 && (
            <li>Agent instruction files in this PR: {facts.agent_instruction_files.join(", ")}</li>
          )}
        </ul>
      </Collapsible>
      <Collapsible sectionKey="context-llm" title="LLM analysis at a glance" tone="llm" sections={sections}>
        <ul className="m-0 list-disc pl-5">
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

const CATEGORY: Record<NotAnalysedCategory, string> = {
  tests: "Tests",
  docs_comments: "Docs & comments",
  generated_mechanical: "Mechanical & generated",
};

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
          {row.category === "generated_mechanical" && (
            <p>{plural(counters.dependencies_changed.length, "dependency", "dependencies")} changed.</p>
          )}
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
    text = `${plural(review.comments.length, "comment")} in your GitHub pending review. Edit and delete act on GitHub; you submit the review on GitHub.`;
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
