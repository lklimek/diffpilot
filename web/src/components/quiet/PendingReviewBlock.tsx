import type { PendingComment, PendingReview } from "../../api/generated/types";
import { PendingCommentItem } from "../comment/PendingCommentItem";
import { PENDING_INCOMPLETE } from "../section/format";
import { Tooltip } from "../tooltip/Tooltip";
import { Note, QuietCard } from "./QuietCard";

interface PendingReviewBlockProps {
  /** The user's pending review on GitHub. */
  review: PendingReview;
  /** The pull request page on GitHub (`PrSummary.html_url`). */
  prUrl: string;
  /** Replaces a comment's text on GitHub; resolves when saved, rejects with an `Error` to keep the draft. */
  onEdit?: (commentId: string, body: string) => Promise<void>;
  /** Deletes a comment on GitHub; resolves when deleted, rejects with an `Error` to show the failure. */
  onDelete?: (commentId: string) => Promise<void>;
  /** The step of the tour that shows a comment's line (label, open it there), or null when none does. */
  placeOf?: (comment: PendingComment) => { label: string; open: () => void } | null;
}

const LINK =
  "cursor-pointer rounded-md border border-border bg-surface px-2 py-0.5 text-xs text-text hover:border-accent";

/** Link back to the step that shows the comment's line. */
function Place({ place }: { place: { label: string; open: () => void } | null }) {
  if (!place)
    return <span className="self-center text-xs text-muted">No step of this tour shows this line.</span>;
  return (
    <Tooltip tip="Opens the step that shows this line, with the comment open there.">
      <button type="button" onClick={place.open} className={LINK}>
        Show in {place.label}
      </button>
    </Tooltip>
  );
}

/** The user's pending review on GitHub: comments with location; edit and delete act on GitHub. */
export function PendingReviewBlock({ review, prUrl, onEdit, onDelete, placeOf }: PendingReviewBlockProps) {
  if (!review.commenting_available) {
    return (
      <QuietCard title="Pending review on GitHub">
        <p>Commenting is not available: the GitHub App is not installed on this repository.</p>
      </QuietCard>
    );
  }
  return (
    <QuietCard title={`Pending review on GitHub (${review.comments.length}${review.incomplete ? "+" : ""})`}>
      <Note>Read from your GitHub pending review. Edit and delete act on GitHub; nothing is kept here.</Note>
      {review.incomplete && <p>{PENDING_INCOMPLETE}</p>}
      {review.comments.length === 0 ? (
        !review.incomplete && <p>Your pending review on GitHub has no comments yet.</p>
      ) : (
        <ul className="m-0 grid list-none gap-2 p-0">
          {review.comments.map((c) => (
            <PendingCommentItem key={c.id} comment={c} onEdit={onEdit} onDelete={onDelete}>
              {placeOf && <Place place={placeOf(c)} />}
            </PendingCommentItem>
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
