import type { MouseEvent } from "react";
import type { LineSide, PendingComment } from "../../api/generated/types";
import { Tooltip } from "../tooltip/Tooltip";
import { PendingCommentItem, type PendingCommentItemProps } from "./PendingCommentItem";

/** Key of a displayed line: side and number. */
export const lineKey = (side: LineSide, line: number) => `${side}:${line}`;

/**
 * The user's pending comments of `path` by displayed line (`lineKey`), on a comment's last line. A file comment
 * has no side; it sits on the head line it links to.
 */
export function commentsByLine(
  comments: readonly PendingComment[],
  path: string,
): Map<string, PendingComment[]> {
  const byLine = new Map<string, PendingComment[]>();
  for (const c of comments) {
    if (c.path !== path || c.line === null) continue;
    const key = lineKey(c.side ?? "head", c.line);
    byLine.set(key, [...(byLine.get(key) ?? []), c]);
  }
  return byLine;
}

/** Accessible name of a line's marker and its list: `line 96`, `removed line 54`, `lines 90–96`. */
export function lineName(comments: readonly PendingComment[], removed: boolean): string {
  const [first] = comments;
  if (comments.length === 1 && first && first.start_line !== null && first.start_line !== first.line) {
    return `${removed ? "removed lines" : "lines"} ${first.start_line}–${first.line}`;
  }
  return `${removed ? "removed line" : "line"} ${first?.line ?? ""}`;
}

const BUBBLE = (
  <svg viewBox="0 0 16 16" width="12" height="12" aria-hidden="true" focusable="false">
    <path
      d="M2.5 2h11A1.5 1.5 0 0 1 15 3.5v7a1.5 1.5 0 0 1-1.5 1.5H7.2L4 14.8V12H2.5A1.5 1.5 0 0 1 1 10.5v-7A1.5 1.5 0 0 1 2.5 2z"
      fill="currentColor"
    />
  </svg>
);

interface CommentMarkerProps {
  comments: readonly PendingComment[];
  /** What the comments are on, for the accessible name: `line 96`, or the path for file comments. */
  subject: string;
  /** File comments shown at the top of the file's code (not on a line). */
  file?: boolean;
  open: boolean;
  /** Id of the list it opens. */
  panelId: string;
  onToggle: () => void;
}

/** Speech-bubble marker of the user's pending comments; opens their text in place. */
export function CommentMarker({
  comments,
  subject,
  file = false,
  open,
  panelId,
  onToggle,
}: CommentMarkerProps) {
  const n = comments.length;
  const what = `${file ? "file " : ""}${n === 1 ? "comment" : "comments"}`;
  const name = `Your ${n === 1 ? "" : `${n} `}pending ${what} ${file ? "on" : "at"} ${subject}`;
  const tip =
    n === 1
      ? "Your comment in your GitHub pending review; nothing is published. Opens its text here."
      : "Your comments in your GitHub pending review; nothing is published. Opens their text here.";
  return (
    <Tooltip tip={tip} className="inline-flex">
      <button
        type="button"
        data-mark="comment"
        aria-label={name}
        aria-expanded={open}
        aria-controls={panelId}
        onClick={(event: MouseEvent) => {
          // Inside an LLM highlight a click would also select its remark.
          event.stopPropagation();
          onToggle();
        }}
        className="inline-flex cursor-pointer items-center gap-0.5 rounded-sm border-0 bg-transparent p-0 font-sans text-[10.5px] leading-none text-accent hover:text-text"
      >
        {BUBBLE}
        {n > 1 && <span aria-hidden>{n}</span>}
      </button>
    </Tooltip>
  );
}

interface CommentPanelProps extends Pick<PendingCommentItemProps, "onEdit" | "onDelete"> {
  id: string;
  comments: readonly PendingComment[];
  /** Accessible name of the list. */
  label: string;
}

/** The text of the user's pending comments where they are, with Edit and Delete as in the pending-review block. */
export function CommentPanel({ id, comments, label, onEdit, onDelete }: CommentPanelProps) {
  return (
    <ul
      id={id}
      aria-label={label}
      // Clicks and typing here belong to the comments, not to an LLM highlight around them.
      onClick={(event) => event.stopPropagation()}
      className="sticky left-0 m-0 my-1 ml-[var(--ln)] grid w-[min(70ch,80vw)] list-none gap-1 p-0 font-sans text-[12.5px] whitespace-normal"
    >
      {comments.map((c) => (
        <PendingCommentItem key={c.id} comment={c} onEdit={onEdit} onDelete={onDelete} />
      ))}
    </ul>
  );
}
