import { focusQuestionHeading } from "./cardKeys";
import { useEffect, useRef } from "react";
import type { CommentItem } from "../../api/generated/types";
import { OpenExternal } from "../dashboard/common/OpenExternal";
import { ProvenanceTag } from "../section/ProvenanceTag";
import { clockText } from "../format/time";
import { BotHeading, CommentTitle } from "./CommentTitle";
import { decisionLabel } from "./decisions";
import { DuplicateLinks } from "./DuplicateLinks";
import { GithubCommentText } from "./GithubCommentText";
import { GithubDecision } from "./GithubDecision";
import { itemId, sourceText } from "./parts";
import { commentStatusText } from "./QueueList";

export function CommentOverviewCard({
  item,
  duplicates,
  inPass,
  since,
  now,
}: {
  item: CommentItem;
  duplicates: CommentItem[];
  inPass?: number;
  since?: string | null;
  now: Date;
}) {
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => focusQuestionHeading(heading.current), [item.id]);
  const last = item.decisions.at(-1);
  const imported = item.imported_decision || item.state === "outside";
  const status = commentStatusText(item, inPass, since, now);
  return (
    <article
      aria-label={itemId(item)}
      className="min-w-0 rounded-lg border border-border bg-surface px-4 py-3.5"
    >
      <p className="m-0 text-xs text-muted">
        {sourceText(item)} <OpenExternal href={item.url} label={`Open ${itemId(item)} on GitHub`} />
      </p>
      <h2 ref={heading} tabIndex={-1} className="mt-1 mb-1.5 text-lg font-semibold wrap-anywhere">
        {itemId(item)} · <CommentTitle item={item} />
      </h2>
      <BotHeading item={item} />
      {imported ? (
        <p>
          <GithubDecision item={item} />
        </p>
      ) : (
        last &&
        !last.undone_at && (
          <p>
            {decisionLabel(last)} · {clockText(last.decided_at, now)}
            {item.repeat_of != null && ` · carried from C${item.repeat_of}`}
          </p>
        )
      )}
      {status && (
        <p>
          {status}
          {item.duplicate_of != null && (
            <>
              {" "}
              <ProvenanceTag source="llm" />
            </>
          )}
        </p>
      )}
      <blockquote className="m-0 rounded border-l-4 border-border bg-surface-2 px-2.5 py-1.5 whitespace-pre-wrap wrap-anywhere">
        <GithubCommentText text={item.point?.quote ?? item.body} />
      </blockquote>
      <DuplicateLinks primary={item} duplicates={duplicates} />
    </article>
  );
}
