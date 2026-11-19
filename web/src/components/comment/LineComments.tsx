import type { LineSide, PendingComment } from "../../api/generated/types";
import { Popover } from "../popover/Popover";
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

/** Speech-bubble glyph of comment markers, in the current text colour. */
export const SpeechBubble = () => (
  <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true" focusable="false">
    <path
      d="M2.5 2h11A1.5 1.5 0 0 1 15 3.5v7a1.5 1.5 0 0 1-1.5 1.5H7.2L4 14.8V12H2.5A1.5 1.5 0 0 1 1 10.5v-7A1.5 1.5 0 0 1 2.5 2z"
      fill="currentColor"
    />
  </svg>
);

/** Classes of a gutter marker button: a 24 px target (WCAG 2.5.8) around a larger glyph. */
export const MARKER =
  "relative inline-flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-sm border-0 bg-transparent p-0 font-sans leading-none";

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
  const about =
    n === 1
      ? "Your comment in your GitHub pending review; nothing is published."
      : "Your comments in your GitHub pending review; nothing is published.";
  return (
    <Popover
      label={name}
      className="inline-flex"
      content={(pinned) => (
        <>
          {comments.map((c) => (
            <span key={c.id} className="line-clamp-6 whitespace-pre-wrap">
              {c.body}
            </span>
          ))}{" "}
          <span className="block text-muted">{about}</span>
          {pinned && (
            <button
              type="button"
              aria-expanded={open}
              aria-controls={panelId}
              onClick={onToggle}
              className="min-h-6 cursor-pointer justify-self-start rounded border border-border bg-surface px-2 text-text hover:border-accent"
            >
              {open
                ? `Hide the ${n === 1 ? "comment" : "comments"} here`
                : `Open the ${n === 1 ? "comment" : "comments"} here`}
            </button>
          )}
        </>
      )}
    >
      <button
        type="button"
        data-mark="comment"
        aria-label={name}
        className={`${MARKER} text-[10px] font-bold text-accent hover:text-text`}
      >
        <SpeechBubble />
        {n > 1 && (
          <span aria-hidden className="absolute right-0 bottom-0 rounded-sm bg-surface px-px">
            {n}
          </span>
        )}
      </button>
    </Popover>
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
