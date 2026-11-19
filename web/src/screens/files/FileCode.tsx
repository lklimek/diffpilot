// The code of one file in the Files view: its changes with context, unchanged lines folded between them and
// loaded by range on Show (brief §7: never a whole file at once), drawn by the shared code view.
import { Fragment, useEffect, useState } from "react";
import type { Annotation, LineRange, LineSide, PrFile } from "../../api/generated/types";
import { useLines } from "../../api/queries";
import type { CodeViewProps } from "../../components/code-view/CodeView";
import { UnchangedFold } from "../../components/code-view/UnchangedFold";
import { displaySide, fileRegions, pathOn } from "../../components/files/model";
import { DiffCode, LinesCode, MAX_RANGE_LINES, rangeChunks, type CodeContext } from "../tour/code";

type View = Pick<
  CodeViewProps,
  "bandOf" | "remarkOwner" | "hunkOf" | "selectedHunk" | "onSelectHunk" | "askLineHint"
>;

interface FileCodeProps extends View {
  file: PrFile;
  ctx: CodeContext;
  /** LLM remarks of every analysed step on the file's side (one ◆ each). */
  annotations: readonly Annotation[];
  /** LLM remarks of steps on the file's removed code (base line numbers). */
  removedAnnotations: readonly Annotation[];
  /** The reader's selected lines (head, or removed lines with side base); null for none. */
  selection: { side: LineSide; from: number; to: number } | null;
  /** Lines to show even when unchanged (a selection or link into folded code). */
  show?: LineRange;
  /** Line scrolled into view when the file opens or the subject changes. */
  revealLine?: number;
  /** "Show old code": the classic diff of every shown range instead. */
  oldCode: boolean;
  /** The file's length on the shown side, once a range of it was read. */
  onTotalLines?: (lines: number) => void;
}

const overlaps = (a: Annotation, r: LineRange) =>
  a.range.start_line <= r.end_line && a.range.end_line >= r.start_line;

/**
 * A remark on removed code as drawn: its range starts on its first removed line in the file, where its one ◆
 * goes (the remark may start on a line kept unchanged, which no view shows).
 */
function fromFirstRemoved(file: PrFile, a: Annotation): Annotation {
  const first = Math.min(
    ...file.hunks
      .flatMap((h) => h.parts)
      .filter((p) => p.side === "base" && overlaps(a, p.range))
      .map((p) => Math.max(p.range.start_line, a.range.start_line)),
  );
  return Number.isFinite(first) && first !== a.range.start_line
    ? { ...a, range: { ...a.range, start_line: first } }
    : a;
}

/** At most one request's worth of lines from `from`. */
const capped = (from: number, to: number): LineRange => ({
  start_line: from,
  end_line: Math.min(to, from + MAX_RANGE_LINES - 1),
});

function RegionCode({
  file,
  side,
  range,
  last,
  onShow,
  ...props
}: Omit<FileCodeProps, "show"> & {
  side: LineSide;
  range: LineRange;
  last: boolean;
  onShow: (range: LineRange) => void;
}) {
  const { ctx, annotations, removedAnnotations, selection, revealLine, oldCode, onTotalLines, ...view } =
    props;
  const query = { path: pathOn(file, side), side, from: range.start_line, to: range.end_line };
  const lines = useLines(ctx.analysisId, query);
  const total = lines.data?.total_lines;
  useEffect(() => {
    if (total !== undefined) onTotalLines?.(total);
  }, [total, onTotalLines]);
  const reveal = revealLine !== undefined && revealLine >= range.start_line && revealLine <= range.end_line;
  return (
    <>
      {oldCode ? (
        <DiffCode analysisId={ctx.analysisId} query={query} comments={ctx} />
      ) : (
        <LinesCode
          ctx={ctx}
          lines={lines}
          annotations={annotations.filter((a) => overlaps(a, range))}
          removedAnnotations={removedAnnotations}
          // Each remark's first line is shown in exactly one region (see FileCode); its ◆ goes there only.
          remarkStartOnly
          revealLine={reveal ? revealLine : undefined}
          selection={selection}
          {...view}
        />
      )}
      {last && lines.data && lines.data.to < lines.data.total_lines && (
        <UnchangedFold
          from={lines.data.to + 1}
          to={lines.data.total_lines}
          onExpand={() => onShow(capped(lines.data!.to + 1, lines.data!.total_lines))}
        />
      )}
    </>
  );
}

/** The file's changed regions in order, with the unchanged lines between them folded. */
export function FileCode({ file, show, ...props }: FileCodeProps) {
  const [opened, setOpened] = useState<readonly LineRange[]>([]);
  const side = displaySide(file);
  // Every remark's first line is shown, so its one ◆ is never folded away.
  const starts = props.annotations.map((a) => ({
    start_line: a.range.start_line,
    end_line: a.range.start_line,
  }));
  const regions = fileRegions(file, [...opened, ...starts, ...(show ? [show] : [])]);
  const removedAnnotations = props.removedAnnotations.map((a) => fromFirstRemoved(file, a));
  const onShow = (range: LineRange) => setOpened((all) => [...all, range]);
  const chunks = regions.flatMap((r) =>
    rangeChunks(pathOn(file, side), side, [r]).map((q) => ({ start_line: q.from, end_line: q.to })),
  );
  return (
    <div className="grid gap-1.5">
      {chunks.map((range, i) => {
        const before = i === 0 ? 0 : chunks[i - 1]!.end_line;
        return (
          <Fragment key={range.start_line}>
            {range.start_line > before + 1 && (
              <UnchangedFold
                from={before + 1}
                to={range.start_line - 1}
                onExpand={() => onShow(capped(before + 1, range.start_line - 1))}
              />
            )}
            <RegionCode
              {...props}
              removedAnnotations={removedAnnotations}
              file={file}
              side={side}
              range={range}
              last={i === chunks.length - 1}
              onShow={onShow}
            />
          </Fragment>
        );
      })}
    </div>
  );
}
