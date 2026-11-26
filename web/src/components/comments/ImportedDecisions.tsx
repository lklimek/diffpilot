import type { CommentItem } from "../../api/generated/types";
import { navigateOnClick, type Navigate } from "../dashboard/common/navigate";
import { StatusMessage } from "../status/StatusMessage";
import { GithubCommentText } from "./GithubCommentText";
import { GithubDecision } from "./GithubDecision";

export function ImportedDecisions({
  count,
  items,
  error,
  expanded,
  href,
  onNavigate,
}: {
  count: number;
  items?: CommentItem[];
  error?: string;
  expanded: boolean;
  href: string;
  onNavigate: Navigate;
}) {
  return (
    <section aria-label="New decisions imported from GitHub" className="my-2 grid gap-1 text-sm">
      <a href={href} onClick={navigateOnClick(href, onNavigate)} className="w-fit underline">
        {expanded
          ? "Hide new imports"
          : `${count} new ${count === 1 ? "decision" : "decisions"} imported from GitHub`}
      </a>
      {expanded &&
        (error ? (
          <StatusMessage kind="error">{error}</StatusMessage>
        ) : !items ? (
          <StatusMessage kind="loading">Loading new imports…</StatusMessage>
        ) : items.length === 0 ? (
          <p>No new GitHub decisions to show.</p>
        ) : (
          <ul className="list-disc pl-5">
            {items.map((item) => {
              const quote = item.decisions.findLast((decision) => !decision.undone_at && decision.imported)
                ?.imported?.quote;
              return (
                <li key={item.id}>
                  C{item.no} · <GithubDecision item={item} />
                  {quote && (
                    <blockquote>
                      <GithubCommentText text={quote} />
                    </blockquote>
                  )}
                </li>
              );
            })}
          </ul>
        ))}
    </section>
  );
}
