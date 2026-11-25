import type { ReviewComment } from "../../api/provisional/comments";
import { navigateOnClick, type Navigate } from "../dashboard/common/navigate";
import { duplicatePrimary, followsPrimary } from "./queueStatus";
import type { CommentItem } from "../../api/generated/types";
import { GithubMark } from "../github/GithubMark";
import { isGithubUrl } from "../github/links";
import { ProvenanceTag } from "../section/ProvenanceTag";
import { itemId } from "./parts";

export function DuplicateLinks({
  primary,
  duplicates,
}: {
  primary: CommentItem;
  duplicates: ReviewComment[];
}) {
  if (!duplicates.length) return null;
  return (
    <p className="my-1 text-xs">
      Also raised as{" "}
      {duplicates.map((item, i) => {
        const label = `${itemId(item)} (${item.author})`;
        return (
          <span key={item.id}>
            {i > 0 && ", "}
            {item.url && isGithubUrl(item.url) ? (
              <a href={item.url} target="_blank" rel="noopener noreferrer">
                {label}
                <GithubMark />
              </a>
            ) : (
              label
            )}
          </span>
        );
      })}
      .{duplicates.every(followsPrimary) && ` Deciding ${itemId(primary)} decides them too.`}{" "}
      <ProvenanceTag source="llm" />
    </p>
  );
}

export function DuplicateOfLink({
  item,
  href,
  onNavigate,
}: {
  item: ReviewComment;
  href?: string;
  onNavigate?: Navigate;
}) {
  const primary = duplicatePrimary(item);
  if (primary == null) return null;
  const label = `Same point as C${primary}`;
  return (
    <span className="text-xs text-muted">
      {href ? (
        <a
          href={href}
          onClick={navigateOnClick(href, onNavigate)}
          className="inline-block min-h-6 content-center text-accent underline"
        >
          {label}
        </a>
      ) : (
        label
      )}{" "}
      <ProvenanceTag source="llm" />
    </span>
  );
}
