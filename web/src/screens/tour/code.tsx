// Code of the review screen: ranges fetched here and drawn by the code-view components (rule A7).
import type { UseQueryResult } from "@tanstack/react-query";
import { useState } from "react";
import type { CodeViewProps } from "../../components/code-view/CodeView";
import { ApiError } from "../../api/client";
import type {
  AnalysisId,
  Annotation,
  AnnotationId,
  CommentTarget,
  Evidence,
  FragmentKind,
  GithubThread,
  LineRange,
  LineSide,
  LinesResponse,
  PendingComment,
  RangeQuery,
  RiskBand,
} from "../../api/generated/types";
import { useDiff, useLines } from "../../api/queries";
import { highlight } from "../../app/highlight";
import { ClassicDiff } from "../../components/code-view/ClassicDiff";
import { CodeView } from "../../components/code-view/CodeView";
import type { MarkerTarget } from "../../components/code-view/markers";
import { StatusMessage } from "../../components/status/StatusMessage";
import { Tooltip } from "../../components/tooltip/Tooltip";

/** "Hide comment and import changes" of the review views, remembered per user (default on). */
export function useHideContextChanges(login: string | null): [boolean, (hide: boolean) => void] {
  const key = `diffpilot.hide-context-changes.${login ?? "anonymous"}`;
  const [hide, setHide] = useState(() => {
    try {
      return localStorage.getItem(key) !== "false";
    } catch {
      return true;
    }
  });
  const change = (next: boolean) => {
    setHide(next);
    try {
      localStorage.setItem(key, String(next));
    } catch {
      /* Storage unavailable; keep this view's preference. */
    }
  };
  return [hide, change];
}

/** Plain text of a failed request. */
export function errorText(error: unknown): string {
  return error instanceof ApiError ? error.message : "Something went wrong. Reload the page.";
}

/** Most lines one range request may ask for (`RangeQuery::MAX_LINES`, http-api.md "Ranges"). */
export const MAX_RANGE_LINES = 2000;

/** Range requests of at most `MAX_RANGE_LINES` lines covering `ranges`, in order; disjoint ranges stay apart. */
export function rangeChunks(path: string, side: LineSide, ranges: readonly LineRange[]): RangeQuery[] {
  return ranges.flatMap((r) => {
    const chunks: RangeQuery[] = [];
    for (let from = r.start_line; from <= r.end_line; from += MAX_RANGE_LINES) {
      chunks.push({ path, side, from, to: Math.min(r.end_line, from + MAX_RANGE_LINES - 1) });
    }
    return chunks;
  });
}

/** What every code view of the screen shares: markers and the comment action. */
export interface CodeContext {
  hideContextChanges?: boolean;
  analysisId: AnalysisId;
  threads: readonly GithubThread[];
  comments: readonly PendingComment[];
  /** The line a comment is being written on (its file only is marked). */
  activeLine: CommentTarget | null;
  /** A line number was activated: comment on it; `lines` are the displayed lines it belongs to. */
  onComment: (target: CommentTarget, lines: LinesResponse) => void;
  /** A line number was activated in the Files view: select it (`extend`: Shift or drag); else `onComment`. */
  onLineSelect?: (target: CommentTarget, how: { extend: boolean }, lines: LinesResponse) => void;
  /** "Ask about this line" (or the selected lines); not offered without it (no step open). */
  onAskLine?: (target: CommentTarget) => void;
  /** A pending comment to show open where its line is displayed. */
  revealComment: string | null;
  /** Edit and delete of a pending comment, on GitHub. */
  onEditComment: (commentId: string, body: string) => Promise<void>;
  onDeleteComment: (commentId: string) => Promise<void>;
  /** The line an evidence link jumped to: highlighted briefly where it is displayed. */
  flash?: Evidence | null;
  /** "Show in Inspector" of a remark or GitHub thread popover. */
  onShowInInspector?: (target: MarkerTarget) => void;
  /** "N.M" of a thread's step when it is not the step shown. */
  otherThreadStep?: (thread: GithubThread) => string | null;
}

interface LinesCodeProps extends Pick<
  CodeViewProps,
  | "bandOf"
  | "remarkOwner"
  | "selection"
  | "hunkOf"
  | "selectedHunk"
  | "onSelectHunk"
  | "removedAnnotations"
  | "remarkStartOnly"
  | "askLineHint"
> {
  ctx: CodeContext;
  revealLine?: number;
  lines: UseQueryResult<LinesResponse>;
  annotations?: readonly Annotation[];
  band?: RiskBand | null;
  kind?: FragmentKind;
  selectedAnnotation?: AnnotationId | null;
  onSelectAnnotation?: (ids: AnnotationId[]) => void;
}

/** A code view of fetched lines, or their loading / error state; truncated removed blocks load on demand. */
export function LinesCode({ ctx, lines, ...view }: LinesCodeProps) {
  // Removed blocks whose rest the user asked for, by their first base line.
  const [opened, setOpened] = useState<readonly number[]>([]);
  if (lines.isPending) return <StatusMessage kind="loading">Loading the code…</StatusMessage>;
  if (lines.isError) {
    return <StatusMessage kind="error">Cannot show the code: {errorText(lines.error)}</StatusMessage>;
  }
  const data = lines.data;
  const active = ctx.activeLine?.path === data.path ? ctx.activeLine : null;
  const flash =
    ctx.flash?.file === data.path &&
    ctx.flash.side === data.side &&
    ctx.flash.line >= data.from &&
    ctx.flash.line <= data.to
      ? ctx.flash.line
      : undefined;
  return (
    <>
      <CodeView
        hideContextChanges={ctx.hideContextChanges}
        lines={data}
        flashLine={flash}
        {...view}
        threads={ctx.threads}
        comments={ctx.comments}
        revealComment={ctx.revealComment}
        onEditComment={ctx.onEditComment}
        onDeleteComment={ctx.onDeleteComment}
        activeLine={active && { side: active.side, line: active.line }}
        highlight={highlight}
        onLineAction={(target, how) =>
          ctx.onLineSelect ? ctx.onLineSelect(target, how, data) : ctx.onComment(target, data)
        }
        onAskLine={ctx.onAskLine}
        onLoadDeleted={(block) => setOpened((all) => [...all, block.base_start])}
        onShowInInspector={ctx.onShowInInspector}
        otherThreadStep={ctx.otherThreadStep}
      />
      {data.deleted
        .filter((block) => opened.includes(block.base_start))
        .map((block) => (
          <ChunkedCode
            key={block.base_start}
            ctx={ctx}
            path={data.path}
            side="base"
            ranges={[
              {
                start_line: block.base_start + block.lines.length,
                end_line: block.base_start + block.total_lines - 1,
              },
            ]}
          />
        ))}
    </>
  );
}

/** Code view of one range of at most `MAX_RANGE_LINES` lines. */
export function RangeCode({ ctx, query }: { ctx: CodeContext; query: RangeQuery }) {
  return <LinesCode ctx={ctx} lines={useLines(ctx.analysisId, query)} />;
}

/** Code of line ranges of any length (impact, quiet rows, removed code): one request-sized part at a time. */
export function ChunkedCode({
  ctx,
  path,
  side,
  ranges,
}: {
  ctx: CodeContext;
  path: string;
  side: LineSide;
  ranges: readonly LineRange[];
}) {
  const chunks = rangeChunks(path, side, ranges);
  const [shown, setShown] = useState(1);
  const next = chunks[shown];
  return (
    <div className="grid gap-1.5">
      {chunks.slice(0, shown).map((query) => (
        <RangeCode key={`${query.from}-${query.to}`} ctx={ctx} query={query} />
      ))}
      {next && (
        <Tooltip
          tip="Loads and shows the next part of these lines; long ranges load in parts."
          className="justify-self-start"
        >
          <button
            type="button"
            onClick={() => setShown((n) => n + 1)}
            className="cursor-pointer rounded border border-border bg-surface px-2 py-0.5 text-[12px] text-text hover:border-accent"
          >
            Show lines {next.from}–{next.to} of {path}
            {side === "base" ? " (base commit)" : ""}
          </button>
        </Tooltip>
      )}
    </div>
  );
}

/** A whole file's classic diff, loaded in bounded ranges on demand. */
export function FileDiffCode({ ctx, path, side }: { ctx: CodeContext; path: string; side: LineSide }) {
  const metadata = useLines(ctx.analysisId, { path, side, from: 1, to: 1 });
  const [shown, setShown] = useState(1);
  if (metadata.isPending) return <StatusMessage kind="loading">Loading the diff…</StatusMessage>;
  if (metadata.isError)
    return <StatusMessage kind="error">Cannot show the diff: {errorText(metadata.error)}</StatusMessage>;
  const chunks = rangeChunks(path, side, [
    { start_line: 1, end_line: Math.max(1, metadata.data.total_lines) },
  ]);
  const next = chunks[shown];
  return (
    <div className="grid gap-1.5">
      {chunks.slice(0, shown).map((query) => (
        <DiffCode key={query.from} analysisId={ctx.analysisId} query={query} comments={ctx} />
      ))}
      {next && (
        <Tooltip tip="Loads the next part of this file's classic diff; nothing is hidden by the comment/import switch.">
          <button
            type="button"
            className="cursor-pointer text-accent underline"
            onClick={() => setShown((n) => n + 1)}
          >
            Show diff lines {next.from}–{next.to}
          </button>
        </Tooltip>
      )}
    </div>
  );
}

/** Classic diff of a range ("Show old code", inspector section 1). */
export function DiffCode({
  analysisId,
  query,
  comments,
}: {
  analysisId: AnalysisId;
  query: RangeQuery;
  /** The reviewer's pending comments with edit and delete, marked on their lines. */
  comments?: Pick<CodeContext, "comments" | "onEditComment" | "onDeleteComment" | "onAskLine">;
}) {
  const diff = useDiff(analysisId, query);
  if (diff.isPending) return <StatusMessage kind="loading">Loading the diff…</StatusMessage>;
  if (diff.isError) {
    return <StatusMessage kind="error">Cannot show the diff: {errorText(diff.error)}</StatusMessage>;
  }
  return (
    <ClassicDiff
      diff={diff.data}
      highlight={highlight}
      comments={comments?.comments}
      onEditComment={comments?.onEditComment}
      onDeleteComment={comments?.onDeleteComment}
      onAskLine={comments?.onAskLine}
    />
  );
}
