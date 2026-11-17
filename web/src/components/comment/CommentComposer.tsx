import { useEffect, useId, useRef, useState, type Ref } from "react";
import type {
  CommentTarget,
  CreateCommentRequest,
  CreateCommentResponse,
  PendingComment,
} from "../../api/generated/types";
import { growField, useAction } from "../inspector/action";
import { GithubLink } from "../inspector/text";
import type { CommentPlacement } from "./placement";

/** The displayed line a comment is written on. */
export interface CommentLine {
  /** Path, side and line(s). */
  target: CommentTarget;
  /** From `commentPlacement(lines, target)`. */
  placement: CommentPlacement;
  /** GitHub link to the line at the analysed commit (offered when the file is not in the PR diff). */
  permalink: string;
}

/** Props of the comment composer in the inspector dock. */
export interface CommentComposerProps {
  /** `PendingReview.commenting_available`: false without the GitHub App on the PR repository. */
  available: boolean;
  /** The line being commented on; null = none chosen yet. */
  line: CommentLine | null;
  /** The user's pending comments (`PendingReview.comments`); those on `line` can be edited and deleted. */
  comments: readonly PendingComment[];
  /** Creates the comment (POST …/pending-review/comments); the key is new per draft, reused on retry. */
  onCreate: (request: CreateCommentRequest) => Promise<CreateCommentResponse>;
  /** Replaces a pending comment's text on GitHub (PATCH). */
  onEdit: (commentId: string, body: string) => Promise<unknown> | void;
  /** Deletes a pending comment on GitHub (DELETE). */
  onDelete: (commentId: string) => Promise<unknown> | void;
  /** Leaves the line (Cancel or Escape); the screen clears the line highlight. */
  onCancel: () => void;
  /** The text field, for the `c` shortcut. */
  textareaRef?: Ref<HTMLTextAreaElement>;
}

const BUTTON =
  "cursor-pointer rounded-md border border-border bg-surface px-2.5 py-1 text-[12.5px] font-semibold text-text hover:border-accent disabled:cursor-wait disabled:opacity-60";
const PRIMARY =
  "cursor-pointer rounded-md border border-accent bg-accent px-2.5 py-1 text-[12.5px] font-semibold text-on-accent disabled:cursor-wait disabled:opacity-60";
const FIELD = "w-full resize-none rounded-md border border-border bg-surface px-2 py-1 font-sans";

/** A version 4 UUID from `crypto.getRandomValues` (`randomUUID` needs a secure context). */
export function newIdempotencyKey(): string {
  const b = crypto.getRandomValues(new Uint8Array(16));
  b[6] = (b[6]! & 0x0f) | 0x40;
  b[8] = (b[8]! & 0x3f) | 0x80;
  const hex = [...b].map((x) => x.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

function where(t: CommentTarget): string {
  const lines = t.start_line !== null && t.start_line !== t.line ? `${t.start_line}–${t.line}` : `${t.line}`;
  return `${t.path}:${lines}${t.side === "base" ? " (removed line)" : ""}`;
}

const onTarget = (c: PendingComment, t: CommentTarget) =>
  c.path === t.path && c.line === t.line && (c.kind === "file" || c.side === t.side);

function assignRef<T>(ref: Ref<T> | undefined, value: T | null) {
  if (typeof ref === "function") ref(value);
  else if (ref) ref.current = value;
}

function OwnComment({
  comment,
  onEdit,
  onDelete,
}: { comment: PendingComment } & Pick<CommentComposerProps, "onEdit" | "onDelete">) {
  const [draft, setDraft] = useState<string | null>(null);
  const [confirming, setConfirming] = useState(false);
  const action = useAction();
  const save = async () => {
    const body = draft?.trim();
    if (!body) return action.setError("Write a comment first.");
    if (await action.run(() => onEdit(comment.id, body))) setDraft(null);
  };
  return (
    <li className="grid gap-1 rounded-md border border-border bg-surface px-2.5 py-1.5">
      {draft === null ? (
        <p className="whitespace-pre-wrap">{comment.body}</p>
      ) : (
        <textarea
          aria-label="Edit comment"
          autoFocus
          rows={3}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          className={FIELD}
        />
      )}
      <div className="flex flex-wrap items-center gap-2">
        {draft !== null ? (
          <>
            <button type="button" disabled={action.busy} onClick={() => void save()} className={BUTTON}>
              Save
            </button>
            <button type="button" onClick={() => setDraft(null)} className={BUTTON}>
              Cancel
            </button>
          </>
        ) : confirming ? (
          <>
            <span>Delete this comment from your GitHub pending review?</span>
            <button
              type="button"
              disabled={action.busy}
              onClick={() => void action.run(() => onDelete(comment.id))}
              className={BUTTON}
            >
              Delete from GitHub
            </button>
            <button type="button" onClick={() => setConfirming(false)} className={BUTTON}>
              Keep
            </button>
          </>
        ) : (
          <>
            <button type="button" onClick={() => setDraft(comment.body)} className={BUTTON}>
              Edit
            </button>
            <button type="button" onClick={() => setConfirming(true)} className={BUTTON}>
              Delete
            </button>
          </>
        )}
      </div>
      {action.error && (
        <p role="alert" className="text-[12px] text-high">
          Not saved: {action.error}
        </p>
      )}
    </li>
  );
}

/** Comment on any displayed line into the user's GitHub pending review (brief §4.3 Dock, ADR-006). */
export function CommentComposer(props: CommentComposerProps) {
  const { available, line, comments, onCreate, onEdit, onDelete, onCancel, textareaRef } = props;
  const id = useId();
  const field = useRef<HTMLTextAreaElement | null>(null);
  const key = useRef<string | null>(null);
  const [text, setText] = useState("");
  const [empty, setEmpty] = useState(false);
  const [done, setDone] = useState<CreateCommentResponse | null>(null);
  const action = useAction();
  const lineKey = line ? `${line.target.side}:${line.target.path}:${line.target.line}` : "";
  const [seenLine, setSeenLine] = useState(lineKey);
  if (seenLine !== lineKey) {
    setSeenLine(lineKey);
    setDone(null);
    setEmpty(false);
    action.setError(null);
  }

  // A newly chosen line takes the focus (click on a line number, or `c`); its draft gets a new key.
  useEffect(() => {
    key.current = null;
    if (lineKey) field.current?.focus();
  }, [lineKey]);

  const heading = (
    <h3 id={`${id}-h`} className="text-[10.5px] font-bold tracking-wide text-muted uppercase">
      Comment
    </h3>
  );
  if (!available) {
    return (
      <section aria-labelledby={`${id}-h`}>
        {heading}
        <p className="text-[12.5px]">
          Commenting is not available for this pull request: the smart-review GitHub App is not installed on
          this repository, so comments cannot be written into your GitHub pending review. Comment on GitHub
          directly.
        </p>
      </section>
    );
  }
  if (line === null) {
    return (
      <section aria-labelledby={`${id}-h`}>
        {heading}
        <p className="text-[12.5px] text-muted">
          Click a line number in the code to comment on that line (c comments on the current line). Any
          displayed line can take a comment, with or without an LLM remark.
        </p>
      </section>
    );
  }

  const location = where(line.target);
  if (line.placement === "not_in_diff") {
    return (
      <section aria-labelledby={`${id}-h`}>
        {heading}
        <p className="text-[12.5px]">
          On <code>{location}</code>. This file is not part of the PR diff, and GitHub does not accept review
          comments on it. Link to the line instead:{" "}
          <GithubLink url={line.permalink} tip="Open the line on GitHub at the analysed commit (new tab)">
            Open the line on GitHub
          </GithubLink>{" "}
          <code className="text-[11px] break-all text-muted">{line.permalink}</code>
        </p>
        <button type="button" onClick={onCancel} className={`${BUTTON} mt-1`}>
          Cancel
        </button>
      </section>
    );
  }

  const mine = comments.filter((c) => onTarget(c, line.target));
  const submit = async () => {
    const body = text.trim();
    if (!body) {
      setEmpty(true);
      field.current?.focus();
      return;
    }
    setEmpty(false);
    setDone(null);
    // One key per draft: a retry of the same text reuses it, so a lost response cannot create a second comment.
    key.current ??= newIdempotencyKey();
    const request: CreateCommentRequest = { idempotency_key: key.current, target: line.target, body };
    let response: CreateCommentResponse | null = null;
    if (await action.run(async () => (response = await onCreate(request)))) {
      key.current = null;
      setText("");
      setDone(response);
      requestAnimationFrame(() => growField(field.current));
    }
  };

  const error = empty ? "Write a comment first." : action.error && `Not added: ${action.error}`;
  return (
    <section aria-labelledby={`${id}-h`}>
      {heading}
      <form
        className="grid gap-1 text-[12.5px]"
        onSubmit={(event) => {
          event.preventDefault();
          void submit();
        }}
      >
        <p className="flex flex-wrap items-center gap-2">
          <span>
            On <code>{location}</code>
          </span>
          <button type="button" onClick={onCancel} className={BUTTON}>
            Cancel
          </button>
        </p>
        {line.placement === "file" && (
          <p className="rounded-md border-l-4 border-l-mv-bd bg-surface px-2 py-1">
            This line is outside the PR diff, so the comment is posted as a file comment whose text links to
            the line.
          </p>
        )}
        <textarea
          ref={(el) => {
            field.current = el;
            assignRef(textareaRef, el);
          }}
          rows={3}
          value={text}
          aria-label={`Comment on ${location}`}
          aria-invalid={empty || undefined}
          aria-describedby={error ? `${id}-e` : undefined}
          placeholder="Your comment"
          onChange={(event) => {
            setText(event.target.value);
            setEmpty(false);
            key.current = null;
            growField(event.target);
          }}
          onKeyDown={(event) => {
            if (event.key === "Escape") onCancel();
          }}
          className={FIELD}
        />
        {error && (
          <p id={`${id}-e`} role="alert" className="text-[12px] text-high">
            {error}
          </p>
        )}
        <p className="flex flex-wrap items-center gap-2">
          <button type="submit" disabled={action.busy} className={PRIMARY}>
            {action.busy ? "Adding…" : "Add to GitHub pending review"}
          </button>
          <span className="text-[11.5px] text-muted">
            The first comment creates the pending review; you submit it on GitHub.
          </span>
        </p>
        {done && (
          <p role="status" className="text-[12px]">
            Added to your GitHub pending review as{" "}
            {done.comment.kind === "file" ? "a file comment that links to the line" : "a line comment"}.
            {done.missed_by_llm_recorded &&
              " Also recorded as missed by the LLM: this changed line has no LLM remark."}
          </p>
        )}
      </form>
      {mine.length > 0 && (
        <h4 id={`${id}-mine`} className="mt-2 text-[11.5px] font-semibold text-muted">
          Your pending comments on this line
        </h4>
      )}
      {mine.length > 0 && (
        <ul aria-labelledby={`${id}-mine`} className="mt-1 grid list-none gap-1.5 p-0 text-[12.5px]">
          {mine.map((c) => (
            <OwnComment key={c.id} comment={c} onEdit={onEdit} onDelete={onDelete} />
          ))}
        </ul>
      )}
    </section>
  );
}
