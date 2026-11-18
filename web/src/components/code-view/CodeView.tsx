import {
  useEffect,
  useRef,
  useId,
  useMemo,
  useState,
  type CSSProperties,
  type KeyboardEvent,
  type MouseEvent,
  type ReactNode,
} from "react";
import type {
  Annotation,
  AnnotationId,
  CommentTarget,
  DeletedBlock,
  FragmentKind,
  GithubThread,
  LineSide,
  LinesResponse,
  PendingComment,
  RiskBand,
} from "../../api/generated/types";
import { CommentMarker, CommentPanel, commentsByLine, lineName } from "../comment/LineComments";
import { useFlashTarget } from "../flash/useFlashTarget";
import { Tooltip } from "../tooltip/Tooltip";
import type { Highlight, Token } from "./highlight";
import { Mark, RemarkMarker, ThreadMarker, type MarkerTarget } from "./markers";
import { buildSegments, type Segment, type Row } from "./rows";
import { CodeText, useTokens } from "./tokens";
import { hiddenLines } from "./visibility";

/** A displayed line: its side and number on that side. */
export interface LineRef {
  side: LineSide;
  line: number;
}

/** Props of the code view; everything is optional except the lines. */
export interface CodeViewProps {
  /** Lines of the range with their deterministic gutter facts (changed, deleted, moved). */
  lines: LinesResponse;
  hideContextChanges?: boolean;
  /** Reveal this line when this range opens, without stealing keyboard focus. */
  revealLine?: number;
  /** Scroll to this line and highlight it briefly (an evidence link jumped here); never hidden. */
  flashLine?: number;
  /** LLM annotations on these lines; only their ranges get the LLM bar. */
  annotations?: readonly Annotation[];
  /** The step's LLM risk band (`assessment.risk`), styling the bar (ADR-008); null = not rated. */
  band?: RiskBand | null;
  /** Step kind (`data-kind` hook); `context` with no changed line in the diff gets the hatch and caption. */
  kind?: FragmentKind;
  /** GitHub review threads of the PR; those on displayed lines get a marker. */
  threads?: readonly GithubThread[];
  /**
   * The reviewer's pending comments; those on displayed lines get a speech-bubble marker that opens their text,
   * file comments whose line is not displayed get one at the top.
   */
  comments?: readonly PendingComment[];
  /** A pending comment to show open (from the pending-review list). */
  revealComment?: string | null;
  /** Replaces a pending comment's text on GitHub; Edit is offered with it. */
  onEditComment?: (commentId: string, body: string) => Promise<void>;
  /** Deletes a pending comment on GitHub; Delete is offered with it. */
  onDeleteComment?: (commentId: string) => Promise<void>;
  /** The line a comment is being written on (highlighted). */
  activeLine?: LineRef | null;
  /** The annotation shown in the inspector; its highlight is outlined. */
  selectedAnnotation?: AnnotationId | null;
  /** Syntax colouring of the displayed lines; plain text until it resolves, and if it fails. */
  highlight?: Highlight;
  /** A highlight was activated (click, or Enter on its label): its annotation ids. */
  onSelectAnnotation?: (ids: AnnotationId[]) => void;
  /** Comment action on a displayed line; without it line numbers are plain text. */
  onLineAction?: (target: CommentTarget) => void;
  /** Load the rest of a deleted block whose `lines` were cut (`lines.length < total_lines`). */
  onLoadDeleted?: (block: DeletedBlock) => void;
  /** "Show in Inspector" of a pinned remark or thread popover; without it the link is not offered. */
  onShowInInspector?: (target: MarkerTarget) => void;
}

// The bar pattern is the colour-independent signal; an unrated remark gets a thin neutral line of its own, so it
// never reads as LOW. The 5 px border stays reserved (transparent) so nothing shifts.
const BANDS: Record<RiskBand | "none", { label: string | null; pattern: string; bar: string; text: string }> =
  {
    high: {
      label: "▲ HIGH",
      pattern: "solid",
      bar: "border-solid border-high-bd bg-high-bg",
      text: "text-high",
    },
    medium: {
      label: "◆ MEDIUM",
      pattern: "dashed",
      bar: "border-dashed border-med-bd bg-med-bg",
      text: "text-med",
    },
    low: {
      label: "● LOW",
      pattern: "dotted",
      bar: "border-dotted border-low-bd bg-low-bg",
      text: "text-low",
    },
    none: {
      label: null,
      pattern: "thin",
      bar: "border-solid border-transparent bg-llm-bg bg-[linear-gradient(var(--color-mark),var(--color-mark))] bg-size-[2px_auto] bg-origin-border bg-no-repeat",
      text: "text-llm",
    },
  };

// Every segment reserves the same 5 px left bar column, so line numbers and code never shift.
const SEGMENT = "border-l-5 min-w-full w-max";
const STRIP = "sticky left-0 block w-max pr-2 pl-[calc(var(--ln)+var(--gutter))] font-sans";

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;
const keyOf = (ref: LineRef) => `${ref.side}:${ref.line}`;

interface LineContext {
  path: string;
  threads: Map<string, GithubThread[]>;
  comments: Map<string, PendingComment[]>;
  /** Whether the comments of a line (or `file`) are shown open. */
  isOpen: (key: string) => boolean;
  toggle: (key: string) => void;
  panelId: (key: string) => string;
  onEditComment?: (commentId: string, body: string) => Promise<void>;
  onDeleteComment?: (commentId: string) => Promise<void>;
  activeKey: string | null;
  tabKey: string | null;
  onLineAction?: (target: CommentTarget) => void;
  onFocusLine: (key: string) => void;
  onShowInInspector?: (target: MarkerTarget) => void;
}

interface LineProps {
  ctx: LineContext;
  side: LineSide;
  line: number;
  text: string;
  tokens: Token[] | undefined;
  row?: Row;
  removed?: boolean;
  /** Inside an LLM highlight: the band tint is the background. */
  highlighted?: boolean;
  /** First line of an LLM highlight: the remarks its marker shows. */
  remarks?: readonly Annotation[];
}

function Line({
  ctx,
  side,
  line,
  text,
  tokens,
  row,
  removed = false,
  highlighted = false,
  remarks,
}: LineProps) {
  const key = keyOf({ side, line });
  const commenting = ctx.activeKey === key;
  const changed = row?.changed ?? false;
  const removedLine = removed || (side === "base" && changed);
  const background = commenting
    ? "bg-accent-bg outline-3 -outline-offset-3 outline-accent"
    : highlighted
      ? ""
      : removedLine
        ? "bg-rm-bg"
        : row?.moved
          ? "bg-mv-bg"
          : "";
  const label = `Comment on ${removedLine ? "removed line" : "line"} ${line}`;
  const mine = ctx.comments.get(key);
  const open = mine !== undefined && ctx.isOpen(key);
  const threads = ctx.threads.get(key);
  // A row with a marker button grows to the button's 24 px target.
  const tall = Boolean(remarks?.length || threads || mine);
  return (
    <>
      <div
        data-line={line}
        data-side={side}
        data-changed={changed || undefined}
        data-moved={row?.moved || undefined}
        data-removed={removed || undefined}
        data-commenting={commenting || undefined}
        className={`grid min-w-full grid-cols-[var(--ln)_var(--gutter)_max-content] ${tall ? "min-h-6 items-center" : ""} ${background}`}
      >
        {ctx.onLineAction ? (
          <button
            type="button"
            data-line-action
            aria-label={label}
            title={label}
            tabIndex={ctx.tabKey === key ? 0 : -1}
            onFocus={() => ctx.onFocusLine(key)}
            onClick={(event: MouseEvent) => {
              event.stopPropagation();
              ctx.onLineAction?.({ path: ctx.path, side, start_line: null, line });
            }}
            className="sticky left-0 cursor-pointer border-0 bg-transparent p-0 pr-2.5 text-right font-[inherit] text-muted hover:text-accent hover:underline"
          >
            {line}
          </button>
        ) : (
          <span className="sticky left-0 pr-2.5 text-right text-muted select-none">{line}</span>
        )}
        <span className="flex items-center gap-px select-none">
          <span className={`h-[1.35em] w-1 ${row?.moved ? "border-l-4 border-double border-mv-bd" : ""}`} />
          {changed ? (
            <span data-mark="changed" className="h-[1.35em] w-2 text-accent">
              <span className="font-bold">{side === "base" ? "−" : "+"}</span>
              <span className="sr-only">changed line</span>
            </span>
          ) : (
            <span className="w-1" />
          )}
          {removed && (
            <Mark
              mark="removed"
              glyph="−"
              text="removed line"
              tip="Removed line: deleted by this pull request, shown from the base commit (from the diff)."
              className="text-[13px] font-bold text-high"
            />
          )}
          {row?.differs && (
            <Mark
              mark="differs"
              glyph="≠"
              text="differs from the moved original"
              tip="This moved line differs from the moved original: it was moved and also edited (from the diff)."
              className="text-[13px] font-bold"
            />
          )}
          {remarks && remarks.length > 0 && (
            <RemarkMarker annotations={remarks} line={line} onShowInInspector={ctx.onShowInInspector} />
          )}
          {threads && (
            <ThreadMarker threads={threads} line={line} onShowInInspector={ctx.onShowInInspector} />
          )}
          {mine && (
            <CommentMarker
              comments={mine}
              subject={lineName(mine, removedLine)}
              open={open}
              panelId={ctx.panelId(key)}
              onToggle={() => ctx.toggle(key)}
            />
          )}
        </span>
        <code className="pr-4 whitespace-pre">
          <CodeText text={text} tokens={tokens} />
          {commenting && (
            <span className="ml-2.5 font-sans text-[11.5px] font-bold text-accent">
              ✎ commenting on this line
            </span>
          )}
        </code>
      </div>
      {open && (
        <CommentPanel
          id={ctx.panelId(key)}
          comments={mine}
          label={`Your pending comments at ${lineName(mine, removedLine)}`}
          onEdit={ctx.onEditComment}
          onDelete={ctx.onDeleteComment}
        />
      )}
    </>
  );
}

function WasIs({ annotations, unrated }: { annotations: readonly Annotation[]; unrated: boolean }) {
  const behaviour = annotations.filter((a) => a.type === "behaviour");
  const pairs = [...new Map(behaviour.map((a) => [`${a.was}\u0000${a.is}`, a])).values()];
  return (
    <>
      {unrated && <span className="block">Risk not rated by the LLM.</span>}
      {pairs.length === 0 && <span className="block">No behaviour annotation on these lines.</span>}
      {pairs.map((a) => (
        <span key={a.id} className="block">
          Was: {a.was} → Is: {a.is}
        </span>
      ))}
    </>
  );
}

interface HighlightRunProps {
  rows: Row[];
  annotations: Annotation[];
  band: RiskBand | null;
  selected: boolean;
  line: (row: Row, highlighted: boolean, remarks?: readonly Annotation[]) => ReactNode;
  onSelect?: (ids: AnnotationId[]) => void;
}

function HighlightRun({ rows, annotations, band, selected, line, onSelect }: HighlightRunProps) {
  const descriptionId = useId();
  const look = BANDS[band ?? "none"];
  const ids = annotations.map((a) => a.id);
  const remark = `LLM remark: ${[...new Set(annotations.map((a) => a.type))].join(", ")}`;
  const text = look.label ? `${look.label} · ${remark}` : remark;
  const description = <WasIs annotations={annotations} unrated={band === null} />;
  const labelClass = `${STRIP} border-0 bg-transparent text-left text-[10.5px] leading-[17px] font-bold ${look.text}`;
  return (
    <Tooltip tip={description} className="block">
      <div
        role="mark"
        data-segment
        data-highlight
        data-band={band ?? "none"}
        data-bar={look.pattern}
        data-selected={selected || undefined}
        onClick={onSelect && (() => onSelect(ids))}
        className={`${SEGMENT} ${look.bar} ${onSelect ? "cursor-pointer" : "cursor-help"} ${
          selected ? "outline-2 -outline-offset-2 outline-accent" : ""
        }`}
      >
        <span id={descriptionId} hidden>
          {description}
        </span>
        {onSelect ? (
          <button
            type="button"
            aria-describedby={descriptionId}
            onClick={(event) => {
              event.stopPropagation();
              onSelect(ids);
            }}
            className={`${labelClass} cursor-pointer`}
          >
            {text}
          </button>
        ) : (
          <span tabIndex={0} aria-describedby={descriptionId} className={labelClass}>
            {text}
          </span>
        )}
        {rows.map((row, i) => line(row, true, i === 0 ? annotations : undefined))}
      </div>
    </Tooltip>
  );
}

interface DeletedRunProps {
  block: DeletedBlock;
  paired?: ReactNode;
  /** Base lines hidden by "hide comment and import changes". */
  hidden: ReadonlySet<number>;
  anchor: string;
  ctx: LineContext;
  /** Token lists of the block's lines, from the view's single highlight call. */
  tokens?: Token[][];
  onLoadDeleted?: (block: DeletedBlock) => void;
}

function DeletedRun({ block, anchor, ctx, tokens, onLoadDeleted, paired, hidden }: DeletedRunProps) {
  const more = plural(block.total_lines - block.lines.length, "more deleted line");
  return (
    <div
      role="group"
      aria-label={`${block.replacement ? "Changed" : "Removed"} code ${anchor}: ${plural(block.total_lines, "line")} from base line ${block.base_start}`}
      data-segment
      className={`${SEGMENT} my-0.5 border-y-2 border-dashed border-transparent border-y-high-bd`}
    >
      <span className={`${STRIP} text-[11px] leading-[17px] font-semibold text-high`}>
        {block.replacement
          ? `Changed: ${plural(block.total_lines, "line")} replaced`
          : `Removed here: ${plural(block.total_lines, "line")} of the base commit`}
      </span>
      {block.lines.map((text, i) =>
        hidden.has(block.base_start + i) ? null : (
          <Line
            key={i}
            ctx={ctx}
            side="base"
            line={block.base_start + i}
            text={text}
            tokens={tokens?.[i]}
            removed
          />
        ),
      )}
      {block.lines.length < block.total_lines && (
        <span className={`${STRIP} py-0.5 text-[11.5px]`}>
          {onLoadDeleted ? (
            <Tooltip tip="Shows the rest of the deleted lines of this block, from the base commit.">
              <button
                type="button"
                onClick={() => onLoadDeleted(block)}
                className="cursor-pointer rounded border border-border bg-surface px-2 text-text hover:border-accent"
              >
                {more}
              </button>
            </Tooltip>
          ) : (
            <span className="text-muted">{more}</span>
          )}
        </span>
      )}
      {paired}
    </div>
  );
}

// Arrow keys, Home and End move between line actions: one tab stop for all lines.
function moveLineFocus(event: KeyboardEvent<HTMLElement>) {
  const target = event.target as HTMLElement;
  if (!("lineAction" in target.dataset)) return;
  const all = [...event.currentTarget.querySelectorAll<HTMLElement>("[data-line-action]")];
  const at = all.indexOf(target);
  const next = (
    { ArrowDown: at + 1, ArrowUp: at - 1, Home: 0, End: all.length - 1 } as Record<string, number>
  )[event.key];
  const button = next === undefined ? undefined : all[next];
  if (!button) return;
  event.preventDefault();
  button.focus();
}

/**
 * Line renderer of one range (brief §4.3 Code view): neutral mark on every changed line, LLM bar by risk band
 * only on annotated ranges (hover: was → is only), `≠` on differing moved lines, markers, removed code inline.
 */
export function CodeView({
  lines,
  hideContextChanges = true,
  revealLine,
  flashLine,
  annotations = [],
  band = null,
  kind,
  threads = [],
  comments = [],
  revealComment = null,
  activeLine = null,
  selectedAnnotation = null,
  highlight,
  onSelectAnnotation,
  onLineAction,
  onLoadDeleted,
  onEditComment,
  onDeleteComment,
  onShowInInspector,
}: CodeViewProps) {
  const id = useId();
  const view = useRef<HTMLDivElement>(null);
  const flash = useMemo(
    () =>
      flashLine === undefined
        ? null
        : { selector: `[data-line="${flashLine}"][data-side="${lines.side}"]`, nonce: 0 },
    [flashLine, lines.side],
  );
  useFlashTarget(view, flash);
  useEffect(() => {
    if (revealLine === undefined) return;
    const frame = requestAnimationFrame(() => {
      view.current
        ?.querySelector<HTMLElement>(`[data-line="${revealLine}"][data-side="${lines.side}"]`)
        ?.scrollIntoView?.({ block: "center", inline: "nearest" });
    });
    return () => cancelAnimationFrame(frame);
  }, [revealLine, lines.path, lines.side, lines.from, lines.to]);
  // Lines (and `file`) the reader opened or closed; the rest follow `revealComment`.
  const [toggled, setToggled] = useState<ReadonlyMap<string, boolean>>(new Map());
  // One highlight call (one budget) for the range and its removed blocks, the range first.
  const fragments = useMemo(() => [lines.lines, ...lines.deleted.map((d) => d.lines)], [lines]);
  const fragmentTokens = useTokens(highlight, lines.path, fragments);
  const tokens = fragmentTokens?.[0];
  const [focusKey, setFocusKey] = useState<string | null>(null);
  const { rows: hidden, removed: hiddenBase } = hiddenLines(lines, hideContextChanges, {
    annotations,
    threads,
    comments,
  });
  if (revealLine !== undefined) hidden.delete(revealLine);
  if (flashLine !== undefined) hidden.delete(flashLine);
  const hiddenRemoved = lines.deleted.reduce(
    (n, block) => n + block.lines.filter((_, i) => hiddenBase.has(block.base_start + i)).length,
    0,
  );
  const hiddenCount = hidden.size + hiddenRemoved;
  const segments = buildSegments(lines, annotations).flatMap<Segment>((segment) => {
    if (segment.kind === "deleted") {
      const paired = segment.paired.filter((row) => !hidden.has(row.line));
      const { block } = segment;
      const hiddenInBlock = (block.context_only ?? []).filter((line) => hiddenBase.has(line)).length;
      if (paired.length === 0 && hiddenInBlock === block.total_lines) return [];
      return [{ ...segment, paired }];
    }
    const rows = segment.rows.filter((row) => !hidden.has(row.line));
    return rows.length ? [{ ...segment, rows }] : [];
  });
  const keys = segments.flatMap((s) =>
    s.kind === "deleted"
      ? [
          ...s.block.lines.map((_, i) => keyOf({ side: "base", line: s.block.base_start + i })),
          ...s.paired.map((row) => keyOf({ side: lines.side, line: row.line })),
        ]
      : s.rows.map((r) => keyOf({ side: lines.side, line: r.line })),
  );
  const byLine = commentsByLine(comments, lines.path);
  // File comments without a displayed line are listed at the top of the file's code.
  const fileComments = comments.filter(
    (c) =>
      c.path === lines.path && c.kind === "file" && !(c.line !== null && keys.includes(`head:${c.line}`)),
  );
  const revealKey =
    revealComment === null
      ? null
      : fileComments.some((c) => c.id === revealComment)
        ? "file"
        : ([...byLine].find(([, cs]) => cs.some((c) => c.id === revealComment))?.[0] ?? null);
  const isOpen = (key: string) => toggled.get(key) ?? key === revealKey;
  const ctx: LineContext = {
    path: lines.path,
    threads: new Map(),
    comments: byLine,
    isOpen,
    toggle: (key) => setToggled((all) => new Map(all).set(key, !isOpen(key))),
    panelId: (key) => `${id}-c-${key}`,
    onEditComment,
    onDeleteComment,
    activeKey: activeLine && keyOf(activeLine),
    tabKey: focusKey && keys.includes(focusKey) ? focusKey : (keys[0] ?? null),
    onLineAction,
    onFocusLine: setFocusKey,
    onShowInInspector,
  };
  for (const t of threads) {
    if (t.path !== lines.path || t.side === null || t.line === null) continue;
    const key = keyOf({ side: t.side, line: t.line });
    ctx.threads.set(key, [...(ctx.threads.get(key) ?? []), t]);
  }
  // The gutter fits the most marker buttons any displayed line has (remark, threads, own comments).
  const remarkKeys = new Set(
    segments.flatMap((s) => {
      const first =
        s.kind === "highlight"
          ? s.rows[0]
          : s.kind === "deleted" && s.annotations.length
            ? s.paired[0]
            : undefined;
      return first ? [keyOf({ side: lines.side, line: first.line })] : [];
    }),
  );
  const buttons = Math.max(
    0,
    ...keys.map((k) => Number(remarkKeys.has(k)) + Number(ctx.threads.has(k)) + Number(byLine.has(k))),
  );

  const maxLine = Math.max(lines.to, ...lines.deleted.map((d) => d.base_start + d.lines.length));
  const base = lines.side === "base";
  const unchangedContext = kind === "context" && lines.changed.length === 0;
  const label = `${lines.path}, lines ${lines.from} to ${lines.to}${base ? ", removed code at the base commit" : ""}`;
  const line = (row: Row, highlighted = false, remarks?: readonly Annotation[]) => (
    <Line
      key={row.line}
      ctx={ctx}
      side={lines.side}
      line={row.line}
      text={row.text}
      tokens={tokens?.[row.index]}
      row={row}
      highlighted={highlighted}
      remarks={remarks}
    />
  );

  return (
    <div
      ref={view}
      role="region"
      aria-label={label}
      tabIndex={0}
      data-kind={kind}
      onKeyDown={moveLineFocus}
      style={
        {
          "--ln": `${String(maxLine).length + 2}ch`,
          "--gutter": `${Math.max(46, 26 + 25 * buttons)}px`,
        } as CSSProperties
      }
      className={`min-w-0 max-w-full overflow-auto rounded-md border border-border bg-code-bg py-0.5 font-mono text-[12.5px] leading-[1.55] ${
        unchangedContext
          ? "bg-[repeating-linear-gradient(135deg,transparent_0_6px,rgb(120_130_150/0.16)_6px_7px)]"
          : ""
      }`}
    >
      {base && <p className={`${STRIP} m-0 text-[11px] text-muted`}>Removed code at the base commit</p>}
      {fileComments.length > 0 && (
        <div className={`${STRIP} m-0 flex items-center gap-1.5 py-0.5 text-[11.5px] text-muted`}>
          <CommentMarker
            comments={fileComments}
            subject={lines.path}
            file
            open={isOpen("file")}
            panelId={ctx.panelId("file")}
            onToggle={() => ctx.toggle("file")}
          />
          {fileComments.length === 1 ? "Your pending file comment" : "Your pending file comments"}
        </div>
      )}
      {fileComments.length > 0 && isOpen("file") && (
        <CommentPanel
          id={ctx.panelId("file")}
          comments={fileComments}
          label={`Your pending file comments on ${lines.path}`}
          onEdit={onEditComment}
          onDelete={onDeleteComment}
        />
      )}
      {hiddenCount > 0 && (
        <p className="px-2 text-xs text-muted">
          {plural(hiddenCount, "comment/import line")} hidden — in the classic diff
        </p>
      )}
      {unchangedContext && (
        <p className={`${STRIP} m-0 text-[11px] font-semibold text-muted`}>
          Unchanged code: no changed lines in this range (from the diff).
        </p>
      )}
      {lines.lines.length === 0 && <p className={`${STRIP} m-0 text-muted`}>No lines in this range.</p>}
      {segments.map((segment) => {
        if (segment.kind === "deleted") {
          const after = segment.block.after_line;
          return (
            <DeletedRun
              key={`d${after}:${segment.block.base_start}`}
              block={segment.block}
              hidden={hiddenBase}
              paired={
                segment.annotations.length > 0 ? (
                  <HighlightRun
                    rows={segment.paired}
                    annotations={segment.annotations}
                    band={band}
                    selected={segment.annotations.some((a) => a.id === selectedAnnotation)}
                    line={line}
                    onSelect={onSelectAnnotation}
                  />
                ) : (
                  segment.paired.map((row) => line(row))
                )
              }
              anchor={after < lines.from ? `before line ${lines.from}` : `after line ${after}`}
              ctx={ctx}
              tokens={fragmentTokens?.[1 + lines.deleted.indexOf(segment.block)] ?? undefined}
              onLoadDeleted={onLoadDeleted}
            />
          );
        }
        if (segment.kind === "rows") {
          return (
            <div key={`r${segment.rows[0]!.line}`} data-segment className={`${SEGMENT} border-transparent`}>
              {segment.rows.map((row) => line(row))}
            </div>
          );
        }
        return (
          <HighlightRun
            key={`h${segment.rows[0]!.line}`}
            rows={segment.rows}
            annotations={segment.annotations}
            band={band}
            selected={segment.annotations.some((a) => a.id === selectedAnnotation)}
            line={line}
            onSelect={onSelectAnnotation}
          />
        );
      })}
    </div>
  );
}
