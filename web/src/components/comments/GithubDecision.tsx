import type { ReviewComment } from "../../api/provisional/comments";
import { OpenExternal } from "../dashboard/common/OpenExternal";
import { dateTimeText } from "../format/time";
import { isGithubUrl } from "../github/links";
import { ProvenanceTag } from "../section/ProvenanceTag";
import { CHOICE_LABEL } from "./decisions";
import { itemId } from "./parts";

export function githubDecisionEvidence(item: ReviewComment): string | null {
  const url = item.imported_decision ? item.imported_decision.evidence_url : item.url;
  return url && isGithubUrl(url) ? url : null;
}

export function GithubDecision({
  item,
  showEvidence = true,
  tip = true,
}: {
  item: ReviewComment;
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
