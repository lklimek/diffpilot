import { useEffect, useId, useRef, useState, type Ref } from "react";
import type {
  CommentTarget,
  CreateCommentRequest,
  CreateCommentResponse,
  PendingComment,
} from "../../api/generated/types";
import { busyTip } from "../button/button";
import { assignRef, codeOf, growField, messageOf, useAction } from "../inspector/action";
import { GithubLink } from "../inspector/text";
import { Tooltip } from "../tooltip/Tooltip";
import type { CommentPlacement } from "./placement";
import { COMMENTING_UNAVAILABLE, COMMENTING_UNAVAILABLE_WHY } from "./unavailable";

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
  /** `PendingReview.commenting_available`: false when the GitHub App is missing or could not be checked. */
  available: boolean;
  /** The line being commented on; null = none chosen yet. */
  line: CommentLine | null;
  /** The user's pending comments (`PendingReview.comments`); those on `line` can be edited and deleted. */
  comments: readonly PendingComment[];
  /** Creates the comment (POST …/pending-review/comments); a failed request is retried with the same key and payload. */
  onCreate: (request: CreateCommentRequest) => Promise<CreateCommentResponse>;
  /** Replaces a pending comment's text on GitHub (PATCH). */
  onEdit: (commentId: string, body: string) => Promise<unknown> | void;
  /** Deletes a pending comment on GitHub (DELETE). */
  onDelete: (commentId: string) => Promise<unknown> | void;
  /** Leaves the line (Close or Escape; an unsent text stays a draft); the screen clears the line highlight. */
  onCancel: () => void;
  /** The text field, for the `c` shortcut. */
  textareaRef?: Ref<HTMLTextAreaElement>;
  /** Posts not confirmed yet, kept by the screen per analysis for the session (survive unmounting); start `{}`. */
  unresolved: UnresolvedPosts;
  /** Updates `unresolved`; takes an updater, so a React state setter fits (`useState<UnresolvedPosts>({})`). */
  onUnresolvedChange: (update: (posts: UnresolvedPosts) => UnresolvedPosts) => void;
  /** Unsent texts by comment target, kept by the screen so leaving a line never loses one. */
  drafts: CommentDrafts;
  /** Where the PR can be analysed again; offered after a refusal because the PR head moved. */
  reanalyseHref?: string;
}

const BUTTON =
  "cursor-pointer rounded-md border border-border bg-surface px-2.5 py-1 text-[12.5px] font-semibold text-text hover:border-accent aria-disabled:cursor-wait aria-disabled:opacity-60";
const PRIMARY =
  "cursor-pointer rounded-md border border-accent bg-accent px-2.5 py-1 text-[12.5px] font-semibold text-on-accent aria-disabled:cursor-wait aria-disabled:opacity-60";

// Tooltips (brief §4 rule 3): what each button does, and what happens on GitHub.
const TIP = {
  add: "Adds this comment to your pending review on GitHub; nothing is published. You submit the review on GitHub.",
  leave:
    "Leaves this line; a text not added yet is kept in this tab and returns when you reopen the line. Nothing is sent to GitHub.",
  draft:
    "Not added to GitHub yet. Kept in this browser tab, also when you leave the line, until you add it or clear the text.",
  leaveOutside: "Leaves this line; nothing is sent to GitHub.",
  reanalyse:
    "Opens Home with this pull request filled in; Check there offers Start analysis for its new head. Ticks are not carried over.",
  retry: "Sends the same comment again with the same key, so GitHub does not add it twice.",
  discard:
    "Makes the text editable again and forgets the failed attempt. It may already be in your pending review on GitHub: check the list below before posting it again.",
  edit: "Edit the text of this comment in your pending review on GitHub.",
  save: "Replaces the text of this comment in your pending review on GitHub; nothing is published.",
  cancelEdit: "Stops editing; the comment on GitHub stays as it was.",
  delete: "Asks for confirmation, then removes this comment from your pending review on GitHub.",
  deleteNow: "Removes this comment from your pending review on GitHub; this cannot be undone here.",
  keep: "Keeps the comment; nothing changes on GitHub.",
} as const;
const FIELD = "w-full resize-none rounded-md border border-border bg-surface px-2 py-1 font-sans";

/** A version 4 UUID from `crypto.getRandomValues` (`randomUUID` needs a secure context). */
export function newIdempotencyKey(): string {
  const b = crypto.getRandomValues(new Uint8Array(16));
  b[6] = (b[6]! & 0x0f) | 0x40;
  b[8] = (b[8]! & 0x3f) | 0x80;
  const hex = [...b].map((x) => x.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

/**
 * API errors of a create that prove nothing was written, or that a repeat cannot succeed (http-api.md
 * § Idempotency): the server's sentence is shown and no Retry offered. Other failures have an unknown outcome.
 */
const DEFINITE_REFUSALS = new Set([
  "comment_published",
  "pr_head_changed",
  "position_rejected",
  "app_not_installed",
  "invalid_request",
  "not_found",
  "unauthenticated",
  "csrf_rejected",
  "login_not_allowed",
  "analysis_not_finished",
]);

/** Identity of a comment target: path, side and the whole line range. */
const identity = (t: CommentTarget) => JSON.stringify([t.path, t.side, t.start_line, t.line]);

const KIND = { line: "a line comment", file: "a file comment that links to the line" } as const;

/** A create request whose outcome is not known yet: in flight (`error` null) or failed. */
export interface UnresolvedPost {
  /** The exact request; Retry sends it again with the same idempotency key. */
  request: CreateCommentRequest;
  /** Why the last attempt failed; null while it is in flight. */
  error: string | null;
}

/** Unresolved posts by comment target (path, side, start line, line). */
export type UnresolvedPosts = Readonly<Record<string, UnresolvedPost>>;

/** Unsent comment texts by comment target (path, side, start line, line), kept outside React state. */
export interface CommentDrafts {
  /** The kept text of a target; "" when there is none. */
  get: (key: string) => string;
  /** Keeps the text of a target; "" removes it. */
  set: (key: string, text: string) => void;
}

function without(all: UnresolvedPosts, key: string): UnresolvedPosts {
  const rest = { ...all };
  delete rest[key];
  return rest;
}

function where(t: CommentTarget): string {
  const lines = t.start_line !== null && t.start_line !== t.line ? `${t.start_line}–${t.line}` : `${t.line}`;
  return `${t.path}:${lines}${t.side === "base" ? " (removed line)" : ""}`;
}

const onTarget = (c: PendingComment, t: CommentTarget) =>
  c.path === t.path && c.line === t.line && (c.kind === "file" || c.side === t.side);

function OwnComment({
  comment,
  onEdit,
  onDelete,
}: { comment: PendingComment } & Pick<CommentComposerProps, "onEdit" | "onDelete">) {
  const [draft, setDraft] = useState<string | null>(null);
  const [confirming, setConfirming] = useState(false);
  const action = useAction();
  // comment_published: the server applied the change to the published comment; its sentence says so.
  const published = action.code === "comment_published";
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
          readOnly={action.busy}
          onChange={(e) => setDraft(e.target.value)}
          className={FIELD}
        />
      )}
      <div className="flex flex-wrap items-center gap-2">
        {draft !== null ? (
          <>
            <Tooltip tip={busyTip(TIP.save, action.busy)}>
              <button
                type="button"
                aria-disabled={action.busy || undefined}
                onClick={() => !action.busy && void save()}
                className={BUTTON}
              >
                Save
              </button>
            </Tooltip>
            <Tooltip tip={TIP.cancelEdit}>
              <button type="button" onClick={() => setDraft(null)} className={BUTTON}>
                Cancel
              </button>
            </Tooltip>
          </>
        ) : confirming ? (
          <>
            <span>Delete this comment from your GitHub pending review?</span>
            <Tooltip tip={busyTip(TIP.deleteNow, action.busy)}>
              <button
                type="button"
                aria-disabled={action.busy || undefined}
                onClick={() => !action.busy && void action.run(() => onDelete(comment.id))}
                className={BUTTON}
              >
                Delete from GitHub
              </button>
            </Tooltip>
            <Tooltip tip={TIP.keep}>
              <button type="button" onClick={() => setConfirming(false)} className={BUTTON}>
                Keep
              </button>
            </Tooltip>
          </>
        ) : (
          <>
            <Tooltip tip={TIP.edit}>
              <button type="button" onClick={() => setDraft(comment.body)} className={BUTTON}>
                Edit
              </button>
            </Tooltip>
            <Tooltip tip={TIP.delete}>
              <button type="button" onClick={() => setConfirming(true)} className={BUTTON}>
                Delete
              </button>
            </Tooltip>
          </>
        )}
      </div>
      {action.error && (
        <p role="alert" className="text-[12px] text-high">
          {published ? action.error : `Not saved: ${action.error}`}
        </p>
      )}
    </li>
  );
}

/** Comment on any displayed line into the user's GitHub pending review (brief §4.3 Dock, ADR-006). */
export function CommentComposer(props: CommentComposerProps) {
  const { available, line, onCreate, unresolved, onUnresolvedChange: setUnresolved, drafts } = props;
  const id = useId();
  // Unconfirmed posts live with the screen (props): after a failure the server may have created the comment,
  // so the only ways on are Retry with the same key and payload, or Discard — also after a remount.
  const inFlight = useRef(new Set<string>());

  const heading = (
    <h3 id={`${id}-h`} className="text-[10.5px] font-bold tracking-wide text-muted uppercase">
      Comment
    </h3>
  );
  let body;
  if (!available) {
    body = (
      <p className="text-[12.5px]">
        {COMMENTING_UNAVAILABLE} {COMMENTING_UNAVAILABLE_WHY} Comments cannot be written into your GitHub
        pending review; comment on GitHub directly.
      </p>
    );
  } else if (line === null) {
    body = (
      <p className="text-[12.5px] text-muted">
        Click a line number in the code to comment on that line (c comments on the current line). Any
        displayed line can take a comment, with or without an LLM remark.
      </p>
    );
  } else {
    const key = identity(line.target);
    /**
     * Sends `request` once at a time per target; resolves to the response, to `{ settled }` when the server
     * says nothing of the attempt remains, or to null when the outcome is unknown (Retry or Discard).
     */
    const send = async (request: CreateCommentRequest): Promise<SendResult> => {
      // The registry blocks a target in flight across remounts; the ref also catches a second click in one tick.
      if (inFlight.current.has(key) || unresolved[key]?.error === null) return null;
      inFlight.current.add(key);
      setUnresolved((all) => ({ ...all, [key]: { request, error: null } }));
      // While the outcome is unknown no draft is stored: a reload must not offer the text for a second Add.
      drafts.set(key, "");
      try {
        const response = await onCreate(request);
        setUnresolved((all) => without(all, key));
        return response;
      } catch (error) {
        // A definite refusal: nothing to retry (a moved head stays moved); the text stays for the user to keep.
        const code = codeOf(error) ?? "";
        if (DEFINITE_REFUSALS.has(code)) {
          setUnresolved((all) => without(all, key));
          drafts.set(key, request.body);
          return { settled: messageOf(error), headMoved: code === "pr_head_changed" };
        }
        setUnresolved((all) => ({ ...all, [key]: { request, error: messageOf(error) } }));
        return null;
      } finally {
        inFlight.current.delete(key);
      }
    };
    const discard = () => setUnresolved((all) => without(all, key));
    // Draft, error and confirmation belong to one target: another target starts afresh, an old outcome is dropped.
    body = (
      <Draft
        key={key}
        {...props}
        line={line}
        post={unresolved[key] ?? null}
        draft={drafts.get(key)}
        onDraft={(text) => drafts.set(key, text)}
        send={send}
        discard={discard}
      />
    );
  }
  return (
    <section aria-labelledby={`${id}-h`} data-comment-composer>
      {heading}
      {body}
    </section>
  );
}

/** Outcome of a send: the response, a definite failure with the server's sentence, or unknown (null). */
type SendResult = CreateCommentResponse | { settled: string; headMoved: boolean } | null;

interface DraftProps extends Omit<CommentComposerProps, "unresolved" | "drafts"> {
  line: CommentLine;
  /** The unresolved post of this target. */
  post: UnresolvedPost | null;
  /** The kept unsent text of this target. */
  draft: string;
  onDraft: (text: string) => void;
  send: (request: CreateCommentRequest) => Promise<SendResult>;
  discard: () => void;
}

function Draft({
  line,
  comments,
  onEdit,
  onDelete,
  onCancel,
  textareaRef,
  reanalyseHref,
  post: unresolved,
  draft,
  onDraft,
  send,
  discard,
}: DraftProps) {
  const id = useId();
  const field = useRef<HTMLTextAreaElement | null>(null);
  const [text, setTextState] = useState(draft);
  const setText = (value: string) => {
    setTextState(value);
    onDraft(value);
  };
  const [empty, setEmpty] = useState(false);
  const [done, setDone] = useState<CreateCommentResponse | null>(null);
  const [settled, setSettled] = useState<string | null>(null);
  const [headMoved, setHeadMoved] = useState(false);

  // A newly chosen line takes the focus (click on a line number, or `c`).
  useEffect(() => field.current?.focus(), []);

  const location = where(line.target);
  if (line.placement === "not_in_diff") {
    return (
      <>
        <p className="text-[12.5px]">
          On <code>{location}</code>. This file is not part of the PR diff, and GitHub does not accept review
          comments on it. Link to the line instead:{" "}
          <GithubLink url={line.permalink} tip="Open the line on GitHub at the analysed commit (new tab)">
            line {line.target.line} on GitHub
          </GithubLink>
        </p>
        <Tooltip tip={TIP.leaveOutside} className="mt-1 inline-block">
          <button type="button" onClick={onCancel} className={BUTTON}>
            Cancel
          </button>
        </Tooltip>
      </>
    );
  }

  const locked = unresolved !== null;
  const pending = locked && unresolved.error === null;
  const submit = async () => {
    const body = text.trim();
    if (!body) {
      setEmpty(true);
      field.current?.focus();
      return;
    }
    setEmpty(false);
    setDone(null);
    setSettled(null);
    setHeadMoved(false);
    const response = await send({ idempotency_key: newIdempotencyKey(), target: line.target, body });
    settle(response);
  };
  const settle = (response: SendResult) => {
    if (response && "settled" in response) {
      setSettled(response.settled);
      setHeadMoved(response.headMoved);
    } else if (response) finish(response);
  };
  const finish = (response: CreateCommentResponse) => {
    setText("");
    setDone(response);
    requestAnimationFrame(() => growField(field.current));
  };
  const retry = async () => {
    if (!unresolved) return;
    setDone(null);
    settle(await send(unresolved.request));
  };

  const mine = comments.filter((c) => onTarget(c, line.target));
  const error = empty
    ? "Write a comment first."
    : (unresolved?.error && `Not confirmed: ${unresolved.error}`) || settled;
  const preview = line.placement === "line" || line.placement === "file" ? line.placement : null;
  return (
    <>
      {/* The line reference stays in view while the dock scrolls (sticky inside the composer section). */}
      <p className="sticky top-0 z-[1] -mt-0.5 flex flex-wrap items-center gap-2 bg-surface-2 py-0.5 text-[12.5px]">
        <span>
          On <code>{location}</code>
        </span>
        {!locked && text.trim() !== "" && (
          <Tooltip tip={TIP.draft}>
            <span
              tabIndex={0}
              className="cursor-help rounded border border-accent px-1 text-[11px] font-semibold text-accent"
            >
              Draft
            </span>
          </Tooltip>
        )}
        <Tooltip tip={TIP.leave}>
          <button type="button" onClick={onCancel} className={BUTTON}>
            Close
          </button>
        </Tooltip>
      </p>
      <form
        className="grid gap-1 text-[12.5px]"
        onSubmit={(event) => {
          event.preventDefault();
          if (!locked) void submit();
        }}
      >
        {line.placement === "file" && (
          <p className="rounded-md border-l-4 border-l-mv-bd bg-surface px-2 py-1">
            This line is outside the PR diff, so the comment is posted as a file comment whose text links to
            the line.
          </p>
        )}
        {line.placement === "on_post" && (
          <p className="rounded-md border-l-4 border-l-mv-bd bg-surface px-2 py-1">
            Whether this becomes a line comment or a file comment is decided when it is posted.
          </p>
        )}
        <textarea
          ref={(el) => {
            field.current = el;
            assignRef(textareaRef, el);
          }}
          rows={3}
          value={locked ? unresolved.request.body : text}
          readOnly={locked}
          aria-label={`Comment on ${location}`}
          aria-invalid={empty || undefined}
          aria-describedby={error ? `${id}-e` : undefined}
          placeholder="Your comment"
          onChange={(event) => {
            setText(event.target.value);
            setEmpty(false);
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
        {headMoved && !empty && reanalyseHref && (
          <p className="text-[12px]">
            <Tooltip tip={TIP.reanalyse}>
              <a href={reanalyseHref} className="text-accent underline">
                Analyse the new head
              </a>
            </Tooltip>
          </p>
        )}
        {locked && !pending ? (
          <>
            <p className="text-[12px]">
              The comment may already be in your GitHub pending review. Retry sends the same comment with the
              same key, so it is not added twice. Discard makes it editable again; check the list below first,
              because posting it again may add it twice.
            </p>
            <p className="flex flex-wrap items-center gap-2">
              <Tooltip tip={TIP.retry}>
                <button type="button" onClick={() => void retry()} className={PRIMARY}>
                  Retry
                </button>
              </Tooltip>
              <Tooltip tip={TIP.discard}>
                <button
                  type="button"
                  onClick={() => {
                    setText(unresolved.request.body);
                    discard();
                  }}
                  className={BUTTON}
                >
                  Discard
                </button>
              </Tooltip>
            </p>
          </>
        ) : (
          <p className="flex flex-wrap items-center gap-2">
            <Tooltip tip={busyTip(TIP.add, pending)}>
              <button type="submit" aria-disabled={pending || undefined} className={PRIMARY}>
                {pending ? "Adding…" : "Add to GitHub pending review"}
              </button>
            </Tooltip>
            <span className="text-[11.5px] text-muted">
              The first comment creates the pending review; you submit it on GitHub.
            </span>
          </p>
        )}
        {done && (
          <p role="status" className="text-[12px]">
            Added to your GitHub pending review as {KIND[done.comment.kind]}
            {preview && preview !== done.comment.kind && `, not as the ${preview} comment previewed`}.
            {done.missed_by_llm_recorded &&
              " Also recorded as missed by the LLM: this changed line has no LLM remark."}
          </p>
        )}
      </form>
      {mine.length > 0 && (
        <>
          <h4 id={`${id}-mine`} className="mt-2 text-[11.5px] font-semibold text-muted">
            Your pending comments on this line
          </h4>
          <ul aria-labelledby={`${id}-mine`} className="mt-1 grid list-none gap-1.5 p-0 text-[12.5px]">
            {mine.map((c) => (
              <OwnComment key={c.id} comment={c} onEdit={onEdit} onDelete={onDelete} />
            ))}
          </ul>
        </>
      )}
    </>
  );
}
