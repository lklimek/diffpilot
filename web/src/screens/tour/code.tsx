// Code of the review screen: ranges fetched here and drawn by the code-view components (rule A7).
import type { UseQueryResult } from "@tanstack/react-query";
import { useState } from "react";
import { ApiError } from "../../api/client";
import type {
  AnalysisId,
  Annotation,
  AnnotationId,
  CommentTarget,
  DeletedBlock,
  FragmentKind,
  GithubThread,
  LinesResponse,
  PendingComment,
  RangeQuery,
  RiskBand,
} from "../../api/generated/types";
import { useDiff, useLines } from "../../api/queries";
import { highlight } from "../../app/highlight";
import { ClassicDiff } from "../../components/code-view/ClassicDiff";
import { CodeView, type LineRef } from "../../components/code-view/CodeView";
import { StatusMessage } from "../../components/status/StatusMessage";

/** Plain text of a failed request. */
export function errorText(error: unknown): string {
  return error instanceof ApiError ? error.message : "Something went wrong. Reload the page.";
}

/** What every code view of the screen shares: markers and the comment action. */
export interface CodeContext {
  analysisId: AnalysisId;
  threads: readonly GithubThread[];
  comments: readonly PendingComment[];
  activeLine: LineRef | null;
  /** A line number was activated: comment on it; `lines` are the displayed lines it belongs to. */
  onComment: (target: CommentTarget, lines: LinesResponse) => void;
}

interface LinesCodeProps {
  ctx: CodeContext;
  lines: UseQueryResult<LinesResponse>;
  annotations?: readonly Annotation[];
  band?: RiskBand | null;
  kind?: FragmentKind;
  selectedAnnotation?: AnnotationId | null;
  onSelectAnnotation?: (ids: AnnotationId[]) => void;
}

/** A code view of fetched lines, or their loading / error state; truncated removed blocks load on demand. */
export function LinesCode({ ctx, lines, ...view }: LinesCodeProps) {
  const [more, setMore] = useState<DeletedBlock | null>(null);
  if (lines.isPending) return <StatusMessage kind="loading">Loading the code…</StatusMessage>;
  if (lines.isError) {
    return <StatusMessage kind="error">Cannot show the code: {errorText(lines.error)}</StatusMessage>;
  }
  const data = lines.data;
  return (
    <>
      <CodeView
        lines={data}
        {...view}
        threads={ctx.threads}
        comments={ctx.comments}
        activeLine={ctx.activeLine}
        highlight={highlight}
        onLineAction={(target) => ctx.onComment(target, data)}
        onLoadDeleted={setMore}
      />
      {more && (
        <RangeCode
          ctx={ctx}
          query={{
            path: data.path,
            side: "base",
            from: more.base_start + more.lines.length,
            to: more.base_start + more.total_lines - 1,
          }}
        />
      )}
    </>
  );
}

/** Code view of a range (impact, quiet rows, the rest of a removed block). */
export function RangeCode({ ctx, query }: { ctx: CodeContext; query: RangeQuery }) {
  return <LinesCode ctx={ctx} lines={useLines(ctx.analysisId, query)} />;
}

/** Classic diff of a range ("Show old code", inspector section 1). */
export function DiffCode({ analysisId, query }: { analysisId: AnalysisId; query: RangeQuery }) {
  const diff = useDiff(analysisId, query);
  if (diff.isPending) return <StatusMessage kind="loading">Loading the diff…</StatusMessage>;
  if (diff.isError) {
    return <StatusMessage kind="error">Cannot show the diff: {errorText(diff.error)}</StatusMessage>;
  }
  return <ClassicDiff diff={diff.data} highlight={highlight} />;
}
