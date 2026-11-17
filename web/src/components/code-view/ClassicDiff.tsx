import { Fragment, useId, useMemo, useState } from "react";
import type {
  DiffHunk,
  DiffLine,
  DiffLineKind,
  DiffResponse,
  LineEnding,
  PendingComment,
} from "../../api/generated/types";
import { CommentMarker, CommentPanel, commentsByLine, lineKey, lineName } from "../comment/LineComments";
import type { Highlight, Token } from "./highlight";
import { CodeText, useTokens } from "./tokens";

/** Props of the classic diff ("Show old code", inspector section 1). */
export interface ClassicDiffProps {
  /** The diff of a range (`GET …/diff`). */
  diff: DiffResponse;
  /** Syntax colouring, as in the code view. */
  highlight?: Highlight;
  /** The reviewer's pending comments; those on shown lines get a marker that opens their text. */
  comments?: readonly PendingComment[];
  /** Replaces a pending comment's text on GitHub; Edit is offered with it. */
  onEditComment?: (commentId: string, body: string) => Promise<void>;
  /** Deletes a pending comment on GitHub; Delete is offered with it. */
  onDeleteComment?: (commentId: string) => Promise<void>;
}

interface Comments {
  byLine: Map<string, PendingComment[]>;
  open: ReadonlySet<string>;
  toggle: (key: string) => void;
  panelId: (key: string) => string;
  onEdit?: (commentId: string, body: string) => Promise<void>;
  onDelete?: (commentId: string) => Promise<void>;
}

/** The comments of a diff line: on its new line (head) or, for a removed line, its old line (base). */
function lineComments(line: DiffLine, comments: Comments): [string, PendingComment[], boolean] | null {
  for (const [side, number] of [
    ["head", line.new_line],
    ["base", line.old_line],
  ] as const) {
    if (number === null) continue;
    const key = lineKey(side, number);
    const mine = comments.byLine.get(key);
    if (mine) return [key, mine, side === "base"];
  }
  return null;
}

const KINDS: Record<DiffLineKind, { sign: string; text: string; look: string }> = {
  context: { sign: " ", text: "unchanged", look: "" },
  removed: { sign: "−", text: "removed", look: "bg-rm-bg" },
  added: { sign: "+", text: "added", look: "bg-ad-bg" },
};

const ENDINGS: Record<LineEnding, string> = { crlf: "CRLF", no_newline: "no newline at end of file" };

/** The line ending, when it is worth showing: on a changed line (a change of only the ending would otherwise look
 * like no change) and a missing final newline anywhere. */
function ending(line: DiffLine): string | null {
  if (!line.eol || (line.kind === "context" && line.eol === "crlf")) return null;
  return ENDINGS[line.eol];
}

function Hunk({
  hunk,
  tokens,
  comments,
}: {
  hunk: DiffHunk;
  tokens: Token[][] | undefined;
  comments: Comments;
}) {
  return (
    <>
      <div className="sticky left-0 w-max px-2 text-muted">{hunk.header}</div>
      {hunk.lines.map((l, i) => {
        const kind = KINDS[l.kind];
        const eol = ending(l);
        const mine = lineComments(l, comments);
        const open = mine !== null && comments.open.has(mine[0]);
        return (
          <Fragment key={i}>
            <div
              data-diff-line={l.kind}
              className={`grid min-w-full grid-cols-[5ch_5ch_2ch_max-content] ${kind.look}`}
            >
              <span className="pr-1 text-right text-muted select-none">{l.old_line}</span>
              <span className="pr-1 text-right text-muted select-none">{l.new_line}</span>
              <span className="text-center font-bold select-none">
                <span aria-hidden>{kind.sign}</span>
                <span className="sr-only">{kind.text}</span>
              </span>
              <code className="pr-4 whitespace-pre">
                <CodeText text={l.text} tokens={tokens?.[i]} />
                {eol && <span className="ml-2 font-sans text-[11px] text-muted select-none">({eol})</span>}
                {mine && (
                  <span className="ml-2.5 align-middle">
                    <CommentMarker
                      comments={mine[1]}
                      subject={lineName(mine[1], mine[2])}
                      open={open}
                      panelId={comments.panelId(mine[0])}
                      onToggle={() => comments.toggle(mine[0])}
                    />
                  </span>
                )}
              </code>
            </div>
            {mine && open && (
              <CommentPanel
                id={comments.panelId(mine[0])}
                comments={mine[1]}
                label={`Your pending comments at ${lineName(mine[1], mine[2])}`}
                onEdit={comments.onEdit}
                onDelete={comments.onDelete}
              />
            )}
          </Fragment>
        );
      })}
    </>
  );
}

/** Classic diff of a range: hunks with old and new line numbers; removed and added lines named for screen readers. */
export function ClassicDiff({
  diff,
  highlight,
  comments = [],
  onEditComment,
  onDeleteComment,
}: ClassicDiffProps) {
  const id = useId();
  const [open, setOpen] = useState<ReadonlySet<string>>(new Set());
  const shared: Comments = {
    byLine: commentsByLine(comments, diff.path),
    open,
    toggle: (key) =>
      setOpen((all) => {
        const next = new Set(all);
        if (!next.delete(key)) next.add(key);
        return next;
      }),
    panelId: (key) => `${id}-c-${key}`,
    onEdit: onEditComment,
    onDelete: onDeleteComment,
  };
  // One highlight call (one budget) for all hunks.
  const fragments = useMemo(() => diff.hunks.map((h) => h.lines.map((l) => l.text)), [diff]);
  const tokens = useTokens(highlight, diff.path, fragments);
  return (
    <div
      role="region"
      aria-label={`Classic diff of ${diff.path}`}
      tabIndex={0}
      className="overflow-auto rounded-md border border-border bg-code-bg py-0.5 font-mono text-[12.5px] leading-[1.55]"
    >
      {diff.hunks.length === 0 && <p className="m-0 px-2 font-sans text-muted">No change in this range.</p>}
      <div className="w-max min-w-full">
        {diff.hunks.map((hunk, i) => (
          <Hunk key={hunk.header} hunk={hunk} tokens={tokens?.[i] ?? undefined} comments={shared} />
        ))}
      </div>
    </div>
  );
}
