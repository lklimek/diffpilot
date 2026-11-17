import { useState } from "react";
import type { PendingComment, PendingReview } from "../../api/generated/types";
import { Tooltip } from "../tooltip/Tooltip";
import { Note, QuietCard } from "./QuietCard";

interface PendingReviewBlockProps {
  /** The user's pending review on GitHub. */
  review: PendingReview;
  /** The pull request page on GitHub (`PrSummary.html_url`). */
  prUrl: string;
  /** Replaces a comment's text on GitHub; no Edit button without it. */
  onEdit?: (commentId: string, body: string) => void;
  /** Deletes a comment on GitHub; no Delete button without it. */
  onDelete?: (commentId: string) => void;
}

const BUTTON =
  "cursor-pointer rounded-md border border-border bg-surface px-2 py-0.5 text-xs text-text hover:border-accent disabled:cursor-not-allowed disabled:opacity-55";

function location(c: PendingComment): string {
  const lines = c.start_line !== null && c.line !== null ? `${c.start_line}–${c.line}` : `${c.line ?? ""}`;
  if (c.kind === "file") {
    if (c.line === null) return `${c.path} (file comment)`;
    const what = c.start_line !== null ? "lines" : "line";
    return `${c.path} (file comment, links to ${what} ${lines})`;
  }
  return `${c.path}:${lines}${c.side === "base" ? " (removed line)" : ""}`;
}

function Comment({
  comment,
  onEdit,
  onDelete,
}: { comment: PendingComment } & Partial<PendingReviewBlockProps>) {
  const [draft, setDraft] = useState<string | null>(null);
  const where = location(comment);
  return (
    <li className="grid gap-1 rounded-md border border-border bg-surface-2 px-2.5 py-2">
      <code className="text-xs font-semibold">{where}</code>
      {draft === null ? (
        <p className="whitespace-pre-wrap">{comment.body}</p>
      ) : (
        <textarea
          aria-label={`Edit comment on ${where}`}
          value={draft}
          rows={3}
          onChange={(e) => setDraft(e.target.value)}
          className="w-full rounded-md border border-border bg-surface px-2 py-1 font-sans"
        />
      )}
      <div className="flex gap-2">
        {draft === null ? (
          <>
            {onEdit && (
              <button
                type="button"
                className={BUTTON}
                aria-label={`Edit comment on ${where}`}
                onClick={() => setDraft(comment.body)}
              >
                Edit
              </button>
            )}
            {onDelete && (
              <button
                type="button"
                className={BUTTON}
                aria-label={`Delete comment on ${where}`}
                onClick={() => onDelete(comment.id)}
              >
                Delete
              </button>
            )}
          </>
        ) : (
          <>
            <button
              type="button"
              className={BUTTON}
              disabled={draft.trim() === ""}
              onClick={() => {
                onEdit?.(comment.id, draft);
                setDraft(null);
              }}
            >
              Save
            </button>
            <button type="button" className={BUTTON} onClick={() => setDraft(null)}>
              Cancel
            </button>
          </>
        )}
      </div>
    </li>
  );
}

/** The user's pending review on GitHub: comments with location; edit and delete act on GitHub. */
export function PendingReviewBlock({ review, prUrl, onEdit, onDelete }: PendingReviewBlockProps) {
  if (!review.commenting_available) {
    return (
      <QuietCard title="Pending review on GitHub">
        <p>Commenting is not available: the GitHub App is not installed on this repository.</p>
      </QuietCard>
    );
  }
  return (
    <QuietCard title={`Pending review on GitHub (${review.comments.length})`}>
      <Note>Read from your GitHub pending review. Edit and delete act on GitHub; nothing is kept here.</Note>
      {review.comments.length === 0 ? (
        <p>Your pending review on GitHub has no comments yet.</p>
      ) : (
        <ul className="m-0 grid list-none gap-2 p-0">
          {review.comments.map((c) => (
            <Comment key={c.id} comment={c} onEdit={onEdit} onDelete={onDelete} />
          ))}
        </ul>
      )}
      {prUrl.startsWith("https://github.com/") && (
        <p>
          <Tooltip tip="Open the pull request's files on GitHub, where you submit the review (new tab)">
            <a href={`${prUrl}/files`} target="_blank" rel="noopener noreferrer" className="font-semibold">
              Finish on GitHub ↗
            </a>
          </Tooltip>
        </p>
      )}
    </QuietCard>
  );
}
