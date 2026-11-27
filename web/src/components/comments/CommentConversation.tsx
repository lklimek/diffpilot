import type { ReactNode } from "react";
import type { AgentAnnotation, GithubThreadComment } from "../../api/generated/types";
import { dateTimeText } from "../format/time";
import { GithubLink } from "../inspector/text";
import { GithubThreadBody } from "./GithubThreadPreview";
import { AgentAnnotations } from "./AgentAnnotations";

/** GitHub's comment fragment remains the same when the repository path changes. */
function commentKey(url: string) {
  const fragment = url.split("#")[1];
  return fragment && /^(discussion_r|issuecomment-|pullrequestreview-)\d+$/.test(fragment) ? fragment : url;
}

/** Displays the source and comments of a conversation without decision controls. */
export function CommentConversation({
  heading,
  source,
  comments,
  annotations,
}: {
  heading: ReactNode;
  source: ReactNode;
  comments: readonly GithubThreadComment[];
  annotations?: readonly AgentAnnotation[];
}) {
  const byUrl = new Map<string, AgentAnnotation[]>();
  for (const annotation of annotations ?? []) {
    const key = commentKey(annotation.target_url);
    const list = byUrl.get(key) ?? [];
    list.push(annotation);
    byUrl.set(key, list);
  }
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
            {comment.url && (
              <AgentAnnotations annotations={byUrl.get(commentKey(comment.url))} showTarget={false} />
            )}
          </li>
        ))}
      </ol>
    </article>
  );
}
