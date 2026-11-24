import type { ProposedDecision } from "../../api/generated/types";

/** Preserve the model's original target as text; it is not an executable action. */
export function ProposalText({ proposal }: { proposal: ProposedDecision | null | undefined }) {
  if (!proposal) return <span>Proposal unavailable</span>;
  return (
    <span>
      {proposal.kind.replaceAll("_", " ")}
      {proposal.target_pr !== null && ` · PR #${proposal.target_pr}`}
      {proposal.target_url && ` · ${proposal.target_url}`}
      {proposal.title && ` · ${proposal.title}`}
      {proposal.fixed_in && ` · fixed at ${proposal.fixed_in}`}
    </span>
  );
}
