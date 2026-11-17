import { useState } from "react";
import type { PendingComment, PendingReview } from "../../api/generated/types";
import { BUSY_NOTE, busyTip } from "../button/button";
import { codeOf } from "../inspector/action";
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
}

const BUTTON =
  "cursor-pointer rounded-md border border-border bg-surface px-2 py-0.5 text-xs text-text hover:border-accent aria-disabled:cursor-not-allowed aria-disabled:opacity-55";

// Tooltips (brief §4 rule 3): what each button does on GitHub.
const TIP = {
  edit: "Edit the text of this comment in your pending review on GitHub.",
  delete: "Removes this comment from your pending review on GitHub right away; this cannot be undone here.",
  save: "Replaces the text of this comment in your pending review on GitHub; nothing is published.",
  empty: "Write the comment text first.",
  cancel: "Stops editing; the comment on GitHub stays as it was.",
} as const;

function location(c: PendingComment): string {
  const lines = c.start_line !== null && c.line !== null ? `${c.start_line}–${c.line}` : `${c.line ?? ""}`;
  if (c.kind === "file") {
    if (c.line === null) return `${c.path} (file comment)`;
    const what = c.start_line !== null ? "lines" : "line";
    return `${c.path} (file comment, links to ${what} ${lines})`;
  }
  return `${c.path}:${lines}${c.side === "base" ? " (removed line)" : ""}`;
}

const reason = (error: unknown) => (error instanceof Error ? error.message : "Something went wrong.");

function Comment({
  comment,
  onEdit,
  onDelete,
}: { comment: PendingComment } & Partial<PendingReviewBlockProps>) {
  const [draft, setDraft] = useState<string | null>(null);
  const [busy, setBusy] = useState<"saving" | "deleting" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const where = location(comment);
  // Unavailable controls stay focusable (aria-disabled) so their tip, which says why, can be read.
  const locked = busy !== null;
  const empty = draft !== null && draft.trim() === "";

  // One request at a time; the editor closes only after GitHub confirmed the save.
  const run = async (kind: "saving" | "deleting", request: () => Promise<void>, failed: string) => {
    setBusy(kind);
    setError(null);
    try {
      await request();
      if (kind === "saving") setDraft(null);
    } catch (e) {
      // comment_published: the change was applied to the published comment; the server's sentence says so.
      setError(codeOf(e) === "comment_published" ? reason(e) : `${failed}: ${reason(e)}`);
    } finally {
      setBusy(null);
    }
  };

  return (
    <li className="grid gap-1 rounded-md border border-border bg-surface-2 px-2.5 py-2">
      <code className="text-xs font-semibold">{where}</code>
      {draft === null ? (
        <p className="max-w-[100ch] whitespace-pre-wrap">{comment.body}</p>
      ) : (
        <textarea
          aria-label={`Edit comment on ${where}`}
          value={draft}
          rows={3}
          readOnly={busy !== null}
          onChange={(e) => setDraft(e.target.value)}
          className="w-full rounded-md border border-border bg-surface px-2 py-1 font-sans"
        />
      )}
      {error && (
        <p role="alert" className="text-xs text-high">
          {error}
        </p>
      )}
      <div className="flex gap-2">
        {draft === null ? (
          <>
            {onEdit && (
              <Tooltip tip={busyTip(TIP.edit, locked)}>
                <button
                  type="button"
                  className={BUTTON}
                  aria-label={`Edit comment on ${where}`}
                  aria-disabled={locked || undefined}
                  onClick={() => {
                    if (locked) return;
                    setError(null);
                    setDraft(comment.body);
                  }}
                >
                  Edit
                </button>
              </Tooltip>
            )}
            {onDelete && (
              <Tooltip tip={busyTip(TIP.delete, locked)}>
                <button
                  type="button"
                  className={BUTTON}
                  aria-label={`Delete comment on ${where}`}
                  aria-disabled={locked || undefined}
                  onClick={() => !locked && void run("deleting", () => onDelete(comment.id), "Not deleted")}
                >
                  {busy === "deleting" ? "Deleting…" : "Delete"}
                </button>
              </Tooltip>
            )}
          </>
        ) : (
          <>
            <Tooltip
              tip={locked ? `${TIP.save} ${BUSY_NOTE}` : empty ? `${TIP.save} ${TIP.empty}` : TIP.save}
            >
              <button
                type="button"
                className={BUTTON}
                aria-disabled={locked || empty || undefined}
                onClick={() =>
                  !locked &&
                  !empty &&
                  onEdit &&
                  void run("saving", () => onEdit(comment.id, draft), "Not saved")
                }
              >
                {busy === "saving" ? "Saving…" : "Save"}
              </button>
            </Tooltip>
            <Tooltip tip={busyTip(TIP.cancel, locked)}>
              <button
                type="button"
                className={BUTTON}
                aria-disabled={locked || undefined}
                onClick={() => {
                  if (locked) return;
                  setError(null);
                  setDraft(null);
                }}
              >
                Cancel
              </button>
            </Tooltip>
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
    <QuietCard title={`Pending review on GitHub (${review.comments.length}${review.incomplete ? "+" : ""})`}>
      <Note>Read from your GitHub pending review. Edit and delete act on GitHub; nothing is kept here.</Note>
      {review.incomplete && <p>{PENDING_INCOMPLETE}</p>}
      {review.comments.length === 0 ? (
        !review.incomplete && <p>Your pending review on GitHub has no comments yet.</p>
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
