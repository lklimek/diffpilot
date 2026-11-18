import { Fragment, useId, useMemo, useState } from "react";
import type {
  CommentTarget,
  DiffHunk,
  DiffLine,
  DiffLineKind,
  DiffResponse,
  LineEnding,
  LineSide,
  PendingComment,
} from "../../api/generated/types";
import { CommentMarker, CommentPanel, commentsByLine, lineKey, lineName } from "../comment/LineComments";
import type { Highlight, Token } from "./highlight";
import { CodeText, useTokens } from "./tokens";
import { CHANGE_LOOK, changeLineText } from "./changeLook";
import { AskLineButton, moveLineFocus, selectedTarget } from "./CodeView";

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
  /** "Ask about this line": a new or unchanged line at the head, a removed line at the base; none without it. */
  onAskLine?: (target: CommentTarget) => void;
}

interface AskLines {
  /** The line whose action is the view's tab stop. */
  tabKey: string;
  onFocus: (key: string) => void;
  onAsk: (target: CommentTarget) => void;
  path: string;
}

// How a diff row names the line it is asked about, for a selection over several rows.
const ASK_ROWS = { selector: "[data-ask-number]", side: "askSide", line: "askNumber" };

/** Where a diff line is asked about: its new line, or its old line when it was removed. */
function askRef(line: DiffLine): { side: LineSide; line: number; key: string } | null {
  const [side, number] =
    line.new_line === null ? (["base", line.old_line] as const) : (["head", line.new_line] as const);
  return number === null ? null : { side, line: number, key: lineKey(side, number) };
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

const KINDS: Record<DiffLineKind, { sign: string; text: string; look: string; signLook: string }> = {
  context: { sign: " ", text: "unchanged", look: "", signLook: "" },
  removed: { ...CHANGE_LOOK.removed, look: CHANGE_LOOK.removed.fill },
  added: { ...CHANGE_LOOK.added, look: CHANGE_LOOK.added.fill },
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
  ask,
}: {
  hunk: DiffHunk;
  tokens: Token[][] | undefined;
  comments: Comments;
  ask?: AskLines;
}) {
  return (
    <>
      <div className="sticky left-0 w-max px-2 text-muted">{hunk.header}</div>
      {hunk.lines.map((l, i) => {
        const kind = KINDS[l.kind];
        const eol = ending(l);
        const mine = lineComments(l, comments);
        const open = mine !== null && comments.open.has(mine[0]);
        const at = askRef(l);
        return (
          <Fragment key={i}>
            <div
              data-diff-line={l.kind}
              data-ask-side={at?.side}
              data-ask-number={at?.line}
              className={`group/line grid min-w-full ${ask ? "grid-cols-[5ch_5ch_1.25rem_2ch_max-content]" : "grid-cols-[5ch_5ch_2ch_max-content]"} ${mine ? "min-h-6 items-center" : ""} ${kind.look}`}
            >
              <span className="pr-1 text-right text-muted select-none">{l.old_line}</span>
              <span className="pr-1 text-right text-muted select-none">{l.new_line}</span>
              {ask &&
                (at ? (
                  <AskLineButton
                    line={at.line}
                    removed={at.side === "base"}
                    tabbable={ask.tabKey === at.key}
                    onFocus={() => ask.onFocus(at.key)}
                    onAsk={(button) =>
                      ask.onAsk(selectedTarget(button, ask.path, at.side, at.line, ASK_ROWS))
                    }
                  />
                ) : (
                  <span />
                ))}
              <span className={`text-center font-bold select-none ${kind.signLook}`}>
                <span aria-hidden>{kind.sign}</span>
                <span className="sr-only">{l.kind === "context" ? kind.text : changeLineText(l.kind)}</span>
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
  onAskLine,
}: ClassicDiffProps) {
  const id = useId();
  const [focusKey, setFocusKey] = useState<string | null>(null);
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
  const keys = diff.hunks.flatMap((h) => h.lines.flatMap((l) => askRef(l)?.key ?? []));
  const ask: AskLines | undefined = onAskLine && {
    tabKey: focusKey && keys.includes(focusKey) ? focusKey : (keys[0] ?? ""),
    onFocus: setFocusKey,
    onAsk: onAskLine,
    path: diff.path,
  };
  return (
    <div
      role="region"
      aria-label={`Classic diff of ${diff.path}`}
      tabIndex={0}
      onKeyDown={moveLineFocus}
      className="min-w-0 max-w-full overflow-auto rounded-md border border-border bg-code-bg py-0.5 font-mono text-[12.5px] leading-[1.55]"
    >
      {diff.hunks.length === 0 && <p className="m-0 px-2 font-sans text-muted">No change in this range.</p>}
      <div className="w-max min-w-full">
        {diff.hunks.map((hunk, i) => (
          <Hunk key={hunk.header} hunk={hunk} tokens={tokens?.[i] ?? undefined} comments={shared} ask={ask} />
        ))}
      </div>
    </div>
  );
}
