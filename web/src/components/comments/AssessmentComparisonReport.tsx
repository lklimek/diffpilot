import { StatusMessage } from "../status/StatusMessage";
import type {
  AssessmentComparison,
  AssessmentComparisonItem,
  AssessmentMethod,
  ComparisonItemStatus,
  DecisionTarget,
} from "../../api/generated/types";
import { ProposalText } from "./ProposalText";

const STATES: Record<ComparisonItemStatus, string> = {
  matched: "Aligned point",
  missing_row: "Missing row",
  duplicate: "Duplicate",
  unmatched: "Unmatched point",
  method_failed: "Method failed",
  head_mismatch: "Different report head; excluded from scoring",
  context: "Context only",
};
function MethodItem({ item }: { item?: AssessmentComparisonItem }) {
  if (!item) return <p>Missing row</p>;
  return (
    <div className="grid gap-1">
      <p>
        {STATES[item.status]}
        {item.duplicate_of && ` · same point as ${item.duplicate_of}`}
      </p>
      <p>
        LLM interpretation: {item.verdict?.replaceAll("_", " ") ?? "Verdict unavailable"} · Native:{" "}
        {item.native_verdict ?? "unavailable"}
      </p>
      <p>
        LLM proposal: <ProposalText proposal={item.proposal} />
      </p>
      <p>
        {item.local_id} · {item.method_version}
      </p>
    </div>
  );
}

function comparisonText(a?: AssessmentComparisonItem, b?: AssessmentComparisonItem): string {
  if (
    a?.status !== "matched" ||
    b?.status !== "matched" ||
    !a.verdict ||
    !b.verdict ||
    !a.proposal ||
    !b.proposal
  )
    return "Incomplete comparison.";
  const sameProposal =
    a.proposal.kind === b.proposal.kind &&
    a.proposal.target_pr === b.proposal.target_pr &&
    a.proposal.target_url === b.proposal.target_url &&
    a.proposal.title === b.proposal.title;
  return `${a.verdict === b.verdict ? "Same" : "Different"} verdicts; ${sameProposal ? "same" : "different"} proposals.`;
}
function targetText(target: DecisionTarget): string | null {
  switch (target.type) {
    case "existing_pr":
      return `PR #${target.number}`;
    case "upstream_issue":
      return target.url;
    default:
      return target.title;
  }
}

/** The report disclosure and its loading, empty and failure states. */
export function AssessmentComparisonPanel({
  expanded,
  report,
  loading,
  error,
  onOpenChange,
}: {
  expanded: boolean;
  report?: AssessmentComparison | null;
  loading: boolean;
  error?: string;
  onOpenChange: (open: boolean) => void;
}) {
  return (
    <details onToggle={(e) => onOpenChange(e.currentTarget.open)}>
      <summary className="min-h-6 cursor-pointer content-center text-sm text-accent">
        Assessment comparison (read-only)
      </summary>
      {expanded &&
        (error ? (
          <StatusMessage kind="error">{error}</StatusMessage>
        ) : loading ? (
          <StatusMessage kind="loading">Loading comparison…</StatusMessage>
        ) : report ? (
          <AssessmentComparisonReport report={report} />
        ) : (
          <p>No assessment comparison yet.</p>
        ))}
    </details>
  );
}

/** Read-only facts and LLM proposals from one frozen pass, including incomplete results. */
export function AssessmentComparisonReport({ report }: { report: AssessmentComparison }) {
  const primary = report.pass.config.primary_method;
  const secondary: AssessmentMethod = primary === "claudius_host" ? "prompt_v4" : "claudius_host";
  const groups = new Map<string, Partial<Record<AssessmentMethod, AssessmentComparisonItem>>>();
  for (const item of report.items) {
    const key = item.comparison_key;
    const group = groups.get(key) ?? {};
    group[item.run.method] = item;
    groups.set(key, group);
  }
  return (
    <section
      aria-label="Assessment comparison report"
      className="my-3 grid gap-3 rounded border border-border bg-surface p-3 text-sm"
    >
      <p>
        {report.pass.input.pr} · Head {report.pass.input.head} · Pass {report.pass.id} · Owner choices as of{" "}
        {report.cutoff}.
      </p>
      <p>
        Read-only comparison. LLM interpretations and proposals are separate from recorded owner choices.
        Comment cards keep the primary for this pass: {primary}.
      </p>
      <p>
        Verdict differences: {report.summary.verdict_disagreements} / {report.summary.verdict_compared}{" "}
        comparable points. Pending owner choices: {report.summary.pending_owner}.
      </p>
      {report.summary.methods.map((score) => (
        <p key={score.method}>
          {score.method}: {score.assessed} assessed; {score.unmatched} unmatched; {score.missing} missing
          rows; {score.missing_verdict} missing verdicts; {score.missing_proposal} missing proposals. Exact
          proposal and target agreement: {score.owner_agreed} / {score.owner_decided} owner-decided points.
          {score.failed && " Method incomplete."}
        </p>
      ))}
      {report.runs.map((run) => (
        <p key={run.reference.method}>
          {run.reference.method} ({run.version}): {run.status}
          {run.failure && ` · ${run.failure}`} · Reported head: {run.reported_head ?? "unavailable"} ·{" "}
          {run.usage_complete ? "Usage" : "Known usage only (incomplete)"}: input {run.usage.input_tokens},
          cached {run.usage.cached_input_tokens}, output {run.usage.output_tokens} tokens · {run.duration_ms}{" "}
          ms. Cached input is included in input.
        </p>
      ))}
      <div className="overflow-x-auto">
        <table aria-label="Assessment items" className="w-full border-collapse text-left">
          <thead>
            <tr>
              {[
                "Item / source",
                `Primary: ${primary}`,
                `Secondary: ${secondary}`,
                "Recorded owner choice",
                "Comparison",
              ].map((label) => (
                <th key={label} scope="col" className="border-b border-border p-2">
                  {label}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {[...groups].map(([key, group]) => {
              const first = group[primary] ?? group[secondary]!;
              const owner = group[primary]?.owner_decision ?? group[secondary]?.owner_decision;
              return (
                <tr key={key} className="align-top">
                  <th scope="row" className="border-b border-border p-2 font-normal">
                    <p>{first.comparison_key}</p>
                    {first.source.kind === "inline" ? (
                      <p>
                        Thread {first.source.thread_id} · version {first.source.version}
                      </p>
                    ) : (
                      <>
                        <p className="break-all">
                          {first.source.url} · version {first.source.version}
                        </p>
                        <blockquote className="whitespace-pre-wrap">{first.source.quote}</blockquote>
                      </>
                    )}
                  </th>
                  <td className="border-b border-border p-2">
                    <MethodItem item={group[primary]} />
                  </td>
                  <td className="border-b border-border p-2">
                    <MethodItem item={group[secondary]} />
                  </td>
                  <td className="border-b border-border p-2">
                    {owner ? (
                      <>
                        {owner.kind.replaceAll("_", " ")} · {owner.target && targetText(owner.target)} ·{" "}
                        {owner.owner} · revision {owner.revision} · {owner.id} · {owner.at}
                      </>
                    ) : (
                      "No recorded owner choice"
                    )}
                  </td>
                  <td className="border-b border-border p-2">
                    {comparisonText(group[primary], group[secondary])}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      {report.items.length === 0 && <p>No comparison items in this pass.</p>}
    </section>
  );
}
