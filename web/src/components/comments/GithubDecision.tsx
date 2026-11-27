import type { CommentItem } from "../../api/generated/types";
import { OpenExternal } from "../dashboard/common/OpenExternal";
import { dateTimeText } from "../format/time";
import { isGithubUrl } from "../github/links";
import { ProvenanceTag } from "../section/ProvenanceTag";
import { CHOICE_LABEL, githubProposal } from "./decisions";
import { itemId } from "./parts";

export function githubDecisionEvidence(item: CommentItem): string | null {
  const url = item.imported_decision ? item.imported_decision.evidence_url : item.url;
  return url && isGithubUrl(url) ? url : null;
}

export function GithubDecision({
  item,
  showEvidence = true,
  tip = true,
}: {
  item: CommentItem;
  showEvidence?: boolean;
  tip?: boolean;
}) {
  const imported = item.imported_decision;
  const actor = imported?.by ?? item.outside?.by ?? item.github.resolved_by;
  const kind = imported?.kind;
  const outcome =
    kind === "reject" ? "Rejected" : !kind || kind === "resolve" ? "Resolved" : CHOICE_LABEL[kind];
  const evidence = githubDecisionEvidence(item);
  return (
    <>
      <ProvenanceTag source="github" tip={tip} /> {outcome} on GitHub
      {actor && ` by ${imported?.kind && !actor.startsWith("@") ? "@" : ""}${actor}`}
      {imported?.at && `, ${dateTimeText(imported.at)}`}
      {imported?.kind ? " (imported)" : ", no reason given"}
      {!imported?.at && item.outside?.seen_at && ` · read ${dateTimeText(item.outside.seen_at)}`}
      {showEvidence && evidence && (
        <>
          {" "}
          <OpenExternal href={evidence} label={`Evidence for ${itemId(item)} on GitHub`} />
        </>
      )}
    </>
  );
}

export function GithubProposal({ item }: { item: CommentItem }) {
  const proposal = githubProposal(item);
  if (!proposal) return null;
  const actor = proposal.by.startsWith("@") ? proposal.by : `@${proposal.by}`;
  return (
    <div className="my-1 flex flex-wrap items-center gap-1.5 text-sm">
      <ProvenanceTag source="github" />
      <span>
        {actor} proposes: {CHOICE_LABEL[proposal.kind]} · {dateTimeText(proposal.at)}
      </span>
      {isGithubUrl(proposal.evidence_url) && (
        <OpenExternal href={proposal.evidence_url} label="Proposal on GitHub" />
      )}
    </div>
  );
}
