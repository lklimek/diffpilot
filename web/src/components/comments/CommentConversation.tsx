import type { ReactNode } from "react";
import type { GithubThreadComment } from "../../api/generated/types";
import { dateTimeText } from "../format/time";
import { GithubLink } from "../inspector/text";
import { GithubThreadBody } from "./GithubThreadPreview";

/** Displays the source and comments of a conversation without decision controls. */
export function CommentConversation({
  heading,
  source,
  comments,
}: {
  heading: ReactNode;
  source: ReactNode;
  comments: readonly GithubThreadComment[];
}) {
  return (
    <article aria-labelledby="comment-conversation-heading" className="grid gap-3">
      <header>
        {source}
        <h1 id="comment-conversation-heading" className="m-0 py-1 text-lg font-semibold">
          {heading}
        </h1>
      </header>
      <ol className="m-0 grid list-none gap-2 p-0">
        {comments.map((comment, index) => (
          <li
            key={comment.url ?? index}
            className="grid gap-1 rounded-lg border border-border bg-surface p-3"
          >
            <div className="flex flex-wrap items-baseline gap-x-2 text-sm">
              <strong>@{comment.author}</strong>
              <time dateTime={comment.created_at} className="text-muted">
                {dateTimeText(comment.created_at)}
              </time>
              {comment.url && (
                <GithubLink url={comment.url} tip="Open the comment on GitHub (new tab)">
                  Open on GitHub
                </GithubLink>
              )}
            </div>
            <GithubThreadBody text={comment.body} />
          </li>
        ))}
      </ol>
    </article>
  );
}
