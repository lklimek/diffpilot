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
import { Popover } from "../popover/Popover";
import { Tooltip } from "../tooltip/Tooltip";
import type { Highlight, Token } from "./highlight";
import { Mark, RemarkMarker, ThreadMarker, type MarkerTarget } from "./markers";
import { FLAG_LOOK, strongestFlag } from "../flag/flags";
import { CHANGE_LOOK, changeLineText, flagBar, REMARK_LOOK } from "./changeLook";
import { foldRuns, type Piece } from "./fold";
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
  /** Action of a line number (comment, or select in the Files view); `extend` with Shift or a drag. */
  onLineAction?: (target: CommentTarget, how: { extend: boolean }) => void;
  /** "Ask about this line" on a displayed line (the selected lines when the line is in a selection); none without it. */
  onAskLine?: (target: CommentTarget) => void;
  /** Load the rest of a deleted block whose `lines` were cut (`lines.length < total_lines`). */
  onLoadDeleted?: (block: DeletedBlock) => void;
  /** "Show in Inspector" of a pinned remark or thread popover; without it the link is not offered. */
  onShowInInspector?: (target: MarkerTarget) => void;
  /** "N.M" of a thread's step when it is not the step shown; its Show in Inspector then names that step. */
  otherThreadStep?: (thread: GithubThread) => string | null;
  /** Rating of each remark's step when the code mixes steps (Files view); a run takes the highest. */
  bandOf?: (annotation: Annotation) => RiskBand | null;
  /** The step a remark belongs to when the code mixes steps ("flow 1 · step 2"), named with it. */
  remarkOwner?: (annotation: Annotation) => string | null;
  /** The reader's selected lines (of `lines.side` unless `side` says base); set (null for none) to select by dragging too. */
  selection?: { side?: LineSide; from: number; to: number } | null;
  /** LLM remarks on removed code (base line numbers): bars over the removed rows and one ◆ each. */
  removedAnnotations?: readonly Annotation[];
  /** A remark's ◆ only where its first line is shown (the file is drawn in several views). */
  remarkStartOnly?: boolean;
  /** What the line's Ask popup says the action does. */
  askLineHint?: string;
  /** The hunk a changed line belongs to; its rows carry the hunk and a click in their code selects it. */
  hunkOf?: (side: LineSide, line: number) => string | null;
  /** The selected hunk (its lines are marked). */
  selectedHunk?: string | null;
  onSelectHunk?: (hunk: string) => void;
}

const BANDS = REMARK_LOOK;

// Every segment reserves the same 5 px left bar column, so line numbers and code never shift.
const SEGMENT = "border-l-5 min-w-full w-max";
const STRIP = "sticky left-0 block w-max pr-2 pl-[calc(var(--ln)+var(--gutter))] font-sans";

const RANK: RiskBand[] = ["low", "medium", "high"];
/** The highest rating of several; null when none is rated. */
const highest = (bands: readonly (RiskBand | null)[]): RiskBand | null =>
  bands.reduce<RiskBand | null>(
    (top, b) => (b !== null && (top === null || RANK.indexOf(b) > RANK.indexOf(top)) ? b : top),
    null,
  );

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;
const keyOf = (ref: LineRef) => `${ref.side}:${ref.line}`;

/** The first and last displayed line of each run of one hunk, in display order. */
function edgesOf(keys: readonly string[], hunkOf?: (side: LineSide, line: number) => string | null) {
  const start = new Set<string>();
  const end = new Set<string>();
  if (!hunkOf) return { start, end };
  const hunks = keys.map((k) => {
    const [side, line] = k.split(":");
    return hunkOf(side as LineSide, Number(line));
  });
  hunks.forEach((h, i) => {
    if (h === null) return;
    if (hunks[i - 1] !== h) start.add(keys[i]!);
    if (hunks[i + 1] !== h) end.add(keys[i]!);
  });
  return { start, end };
}

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
  onLineAction?: (target: CommentTarget, how: { extend: boolean }) => void;
  onAskLine?: (target: CommentTarget) => void;
  onFocusLine: (key: string) => void;
  onShowInInspector?: (target: MarkerTarget) => void;
  unrated: boolean;
  otherThreadStep?: (thread: GithubThread) => string | null;
  remarkOwner?: (annotation: Annotation) => string | null;
  bandOf?: (annotation: Annotation) => RiskBand | null;
  /** Selected lines of the view's side; undefined: no selecting (line numbers comment). */
  selection?: { side: LineSide; from: number; to: number } | null;
  hunkOf?: (side: LineSide, line: number) => string | null;
  selectedHunk?: string | null;
  onSelectHunk?: (hunk: string) => void;
  /** Where hunks begin and end in display order (line keys), for their rules. */
  hunkEdges: { start: ReadonlySet<string>; end: ReadonlySet<string> };
  /** A drag selection started on a line number (and the button is still down). */
  dragging: { current: boolean };
  askLineHint?: string;
}

interface LineProps {
  ctx: LineContext;
  side: LineSide;
  line: number;
  text: string;
  tokens: Token[] | undefined;
  row?: Row;
  removed?: boolean;
  /** The LLM remarks starting on this line: one marker each. */
  remarks?: readonly Annotation[];
}

function Line({ ctx, side, line, text, tokens, row, removed = false, remarks }: LineProps) {
  const key = keyOf({ side, line });
  const commenting = ctx.activeKey === key;
  const changed = row?.changed ?? false;
  const removedLine = removed || (side === "base" && changed);
  // The classic diff's colours: green added, red removed. A moved line is added code there, so it gets the
  // same fill; its double bar says it moved.
  const background = commenting
    ? "bg-accent-bg outline-3 -outline-offset-3 outline-accent"
    : removedLine
      ? CHANGE_LOOK.removed.fill
      : changed
        ? CHANGE_LOOK.added.fill
        : "";
  const selecting = ctx.selection !== undefined;
  const label = `${selecting ? "Select" : "Comment on"} ${removedLine ? "removed line" : "line"} ${line}`;
  const mine = ctx.comments.get(key);
  const open = mine !== undefined && ctx.isOpen(key);
  const threads = ctx.threads.get(key);
  // A row with a marker button grows to the button's 24 px target.
  const tall = Boolean(remarks?.length || threads || mine);
  const selected = ctx.selection?.side === side && ctx.selection.from <= line && line <= ctx.selection.to;
  const hunk = ctx.hunkOf?.(side, line) ?? null;
  const hunkSelected = hunk !== null && hunk === ctx.selectedHunk;
  const hunkStart = hunk !== null && ctx.hunkEdges.start.has(key);
  const hunkEnd = hunk !== null && ctx.hunkEdges.end.has(key);
  // Bars and rules as inset shadows (one property): the hunk's left bar and its top and bottom rules (accent
  // when selected), the selection's own accent frame. The line keeps its change colour under both.
  const accent = "var(--color-accent)";
  const shadows = [
    hunkSelected ? `inset 4px 0 0 ${accent}` : hunk !== null ? "inset 2px 0 0 var(--color-mv-bd)" : "",
    hunkStart ? `inset 0 ${hunkSelected ? 2 : 1}px 0 ${hunkSelected ? accent : "var(--color-mv-bd)"}` : "",
    hunkEnd ? `inset 0 -${hunkSelected ? 2 : 1}px 0 ${hunkSelected ? accent : "var(--color-mv-bd)"}` : "",
    selected ? `inset -3px 0 0 ${accent}, inset 3px 0 0 ${accent}` : "",
    selected && ctx.selection!.from === line ? `inset 0 2px 0 ${accent}` : "",
    selected && ctx.selection!.to === line ? `inset 0 -2px 0 ${accent}` : "",
  ].filter(Boolean);
  const target: CommentTarget = { path: ctx.path, side, start_line: null, line };
  return (
    <>
      <div
        data-line={line}
        data-side={side}
        data-changed={changed || undefined}
        data-moved={row?.moved || undefined}
        data-removed={removed || undefined}
        data-commenting={commenting || undefined}
        data-selected-line={selected || undefined}
        data-hunk={hunk ?? undefined}
        data-hunk-selected={hunkSelected || undefined}
        data-hunk-start={hunkStart || undefined}
        data-hunk-end={hunkEnd || undefined}
        style={shadows.length > 0 ? { boxShadow: shadows.join(", ") } : undefined}
        onClick={
          hunk !== null && ctx.onSelectHunk
            ? (event: MouseEvent) => {
                // A click in the code, not on a control, and not the end of a text selection.
                if ((event.target as Element).closest("button, a")) return;
                if (window.getSelection?.()?.isCollapsed === false) return;
                ctx.onSelectHunk?.(hunk);
              }
            : undefined
        }
        className={`group/line grid min-w-full grid-cols-[var(--ln)_var(--gutter)_max-content] ${tall ? "min-h-6 items-center" : ""} ${selected && !background ? "bg-accent-bg" : background} ${
          hunk !== null && !hunkSelected ? "cursor-pointer" : ""
        }`}
      >
        {ctx.onLineAction ? (
          <button
            type="button"
            data-line-action
            aria-label={label}
            aria-pressed={selecting ? selected : undefined}
            title={
              selecting
                ? `${label}: Shift+click or drag extends; a comment goes on the last selected line`
                : label
            }
            tabIndex={ctx.tabKey === key ? 0 : -1}
            onFocus={() => ctx.onFocusLine(key)}
            onPointerDown={
              selecting
                ? (event) => {
                    ctx.dragging.current = true;
                    ctx.onLineAction?.(target, { extend: event.shiftKey });
                  }
                : undefined
            }
            onPointerOver={
              selecting
                ? (event) => {
                    if (event.buttons === 1 && ctx.dragging.current)
                      ctx.onLineAction?.(target, { extend: true });
                  }
                : undefined
            }
            onClick={(event: MouseEvent) => {
              event.stopPropagation();
              ctx.onLineAction?.(target, { extend: event.shiftKey });
            }}
            className={`sticky left-0 cursor-pointer border-0 bg-transparent p-0 pr-2.5 text-right font-[inherit] hover:text-accent hover:underline ${selected ? "font-bold text-accent" : "text-muted"}`}
          >
            {line}
          </button>
        ) : (
          <span className="sticky left-0 pr-2.5 text-right text-muted select-none">{line}</span>
        )}
        <span className="flex items-center gap-1 select-none">
          <span className={`h-[1.35em] w-1 ${row?.moved ? "border-l-4 border-double border-mv-bd" : ""}`} />
          {/* The sign has its own column: a marker beside it never reads as part of it. */}
          <span className="inline-flex w-2 justify-center">
            {changed && (
              <Mark
                mark="changed"
                glyph={CHANGE_LOOK[side === "base" ? "removed" : "added"].sign}
                text={changeLineText(side === "base" ? "removed" : "added")}
                className={`font-bold ${CHANGE_LOOK[side === "base" ? "removed" : "added"].signLook}`}
              />
            )}
          </span>
          {removed && (
            <Mark
              mark="removed"
              glyph="−"
              text={changeLineText("removed")}
              className="text-[13px] font-bold text-high"
            />
          )}
          {row?.differs && (
            <Mark
              mark="differs"
              glyph="≠"
              text="differs from the moved original"
              className="text-[13px] font-bold"
            />
          )}
          {remarks?.map((a) => (
            <RemarkMarker
              key={a.id}
              annotation={a}
              line={line}
              owner={ctx.remarkOwner?.(a)}
              unrated={ctx.bandOf ? ctx.bandOf(a) === null : ctx.unrated}
              onShowInInspector={ctx.onShowInInspector}
            />
          ))}
          {threads && (
            <ThreadMarker
              threads={threads}
              line={line}
              onShowInInspector={ctx.onShowInInspector}
              otherStep={ctx.otherThreadStep}
            />
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
          {ctx.onAskLine && (
            <AskLineButton
              line={line}
              removed={removedLine}
              tabbable={ctx.tabKey === key}
              onFocus={() => ctx.onFocusLine(key)}
              hint={ctx.askLineHint}
              onAsk={(button) => ctx.onAskLine?.(selectedTarget(button, ctx.path, side, line))}
              className="ml-auto"
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
  annotations: Annotation[];
  band: RiskBand | null;
  /** Names each remark's step in the label (code of several steps). */
  remarkOwner?: (annotation: Annotation) => string | null;
  selected: boolean;
  onSelect?: (ids: AnnotationId[]) => void;
  /** The lines of the range, with removed blocks and fold rows inside it. */
  children: ReactNode;
}

/** One LLM remark range: a single label at its top and a bar along it; the lines keep their own look. */
function HighlightRun({ annotations, band, remarkOwner, selected, onSelect, children }: HighlightRunProps) {
  const descriptionId = useId();
  const look = BANDS[band ?? "none"];
  const flag = strongestFlag(annotations);
  const ids = annotations.map((a) => a.id);
  const owned = (a: Annotation) => {
    const owner = remarkOwner?.(a);
    return owner ? `${a.type} (${owner})` : a.type;
  };
  const kinds = [...new Set(annotations.map(owned))];
  const remark = `${remarkOwner && annotations.length > 1 ? "LLM remarks" : "LLM remark"}: ${kinds.join(", ")}`;
  const text = [flag && FLAG_LOOK[flag].label, look.label, remark].filter(Boolean).join(" · ");
  const description = <WasIs annotations={annotations} unrated={band === null} />;
  const labelClass = `${STRIP} max-w-[100cqw] border-0 bg-transparent text-left text-[10.5px] leading-[17px] font-bold ${flag ? FLAG_LOOK[flag].text : look.text} ${
    selected ? "rounded-sm outline-2 -outline-offset-2 outline-accent" : ""
  }`;
  return (
    <div
      role="mark"
      aria-describedby={descriptionId}
      data-segment
      data-highlight
      data-band={band ?? "none"}
      data-bar={look.pattern}
      data-flag={flag ?? undefined}
      data-selected={selected || undefined}
      onClick={onSelect && (() => onSelect(ids))}
      className={`${SEGMENT} ${flag ? flagBar(flag, band) : look.bar} ${onSelect ? "cursor-pointer" : ""}`}
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
      {children}
    </div>
  );
}

interface DeletedRunProps {
  block: DeletedBlock;
  /** The block's base lines, with fold rows. */
  rows: ReactNode;
  paired?: ReactNode;
  anchor: string;
  /** Inside a remark range: the range's bar column is the left edge. */
  nested?: boolean;
  onLoadDeleted?: (block: DeletedBlock) => void;
}

function DeletedRun({ block, rows, anchor, nested = false, onLoadDeleted, paired }: DeletedRunProps) {
  const more = plural(block.total_lines - block.lines.length, "more deleted line");
  return (
    <div
      role="group"
      aria-label={`${block.replacement ? "Changed" : "Removed"} code ${anchor}: ${plural(block.total_lines, "line")} from base line ${block.base_start}`}
      data-segment
      className={`${nested ? "w-max min-w-full" : `${SEGMENT} border-transparent`} my-0.5 border-y-2 border-dashed border-y-high-bd`}
    >
      <span className={`${STRIP} text-[11px] leading-[17px] font-semibold text-high`}>
        {block.replacement
          ? `Changed: ${plural(block.total_lines, "line")} replaced`
          : `Removed here: ${plural(block.total_lines, "line")} of the base commit`}
      </span>
      {rows}
      {block.lines.length < block.total_lines && (
        <span className={`${STRIP} flex items-center gap-1.5 py-0.5 text-[11.5px]`}>
          <span className="text-muted">{more}</span>
          {onLoadDeleted && (
            <Popover
              label={more}
              hint="Shows the rest of the deleted lines of this block, from the base commit."
              onActivate={() => onLoadDeleted(block)}
              content={() => (
                <span className="block">
                  Click to show the rest of the deleted lines of this block, from the base commit.
                </span>
              )}
            >
              <button
                type="button"
                aria-label={`Show ${more}`}
                className="inline-flex size-6 cursor-pointer items-center justify-center rounded border border-border bg-surface text-text hover:border-accent"
              >
                <span aria-hidden>⋯</span>
              </button>
            </Popover>
          )}
        </span>
      )}
      {paired}
    </div>
  );
}

/** Where rule-covered lines are hidden: what and how many, with Show (or Hide once shown) for just that run. */
function FoldRow({
  count,
  removed,
  shown,
  onToggle,
}: {
  count: number;
  removed: boolean;
  shown: boolean;
  onToggle: () => void;
}) {
  // The diff marks these lines as rule-covered only, not whether each is a comment or an import.
  const what = `${removed ? "removed " : ""}comment/import ${count === 1 ? "line" : "lines"}`;
  return (
    <div
      data-fold
      className="sticky left-0 flex w-max items-center gap-2 py-px pr-2 pl-[calc(var(--ln)+var(--gutter))] font-sans text-[11.5px] text-muted"
    >
      <span>{`${count} ${what} ${shown ? "shown" : "hidden"}`}</span>
      <Tooltip
        tip={
          shown
            ? "Hides these lines here again; the classic diff keeps them."
            : "Shows these lines here with their changed marks; the classic diff always has them. Hide comment and import changes in the top bar shows or hides all."
        }
      >
        <button
          type="button"
          aria-label={shown ? `Hide ${count} ${what}` : `Show ${count} hidden ${what}`}
          onClick={(event) => {
            event.stopPropagation();
            onToggle();
          }}
          className="min-h-6 cursor-pointer rounded border border-border bg-surface px-2 text-text hover:border-accent"
        >
          {shown ? "Hide" : "Show"}
        </button>
      </Tooltip>
    </div>
  );
}

/**
 * The line, or the lines of a selection over several lines of the same view and side that includes it (selected
 * with the mouse before "Ask about this line").
 */
export function selectedTarget(
  button: HTMLElement,
  path: string,
  side: LineSide,
  line: number,
  // How a row names its line: the code view's `data-line` / `data-side`, or another view's attributes.
  rows: { selector: string; side: string; line: string } = {
    selector: "[data-line]",
    side: "side",
    line: "line",
  },
): CommentTarget {
  const single = { path, side, start_line: null, line };
  const selection = window.getSelection();
  const view = button.closest("[role=region]");
  if (!selection || selection.isCollapsed || !view) return single;
  const lineOf = (node: Node | null) => {
    const row = (node instanceof Element ? node : node?.parentElement)?.closest<HTMLElement>(rows.selector);
    return row && view.contains(row) && row.dataset[rows.side] === side
      ? Number(row.dataset[rows.line])
      : null;
  };
  const a = lineOf(selection.anchorNode);
  const b = lineOf(selection.focusNode);
  if (a === null || b === null || a === b) return single;
  const [from, to] = a < b ? [a, b] : [b, a];
  return from <= line && line <= to ? { path, side, start_line: from, line: to } : single;
}

/** The small "Ask about this line" action of a code line; its mark is drawn by CSS, so the row's text stays. */
export function AskLineButton({
  line,
  removed,
  tabbable,
  onFocus,
  onAsk,
  hint = 'Click, then "Ask about this line": puts this line (or the selected lines) into the Ask field. Nothing is sent until you press Ask.',
  className = "",
}: {
  line: number;
  removed: boolean;
  tabbable: boolean;
  onFocus: () => void;
  onAsk: (button: HTMLElement) => void;
  /** What the action does in this view. */
  hint?: string;
  className?: string;
}) {
  const name = `Ask the LLM about ${removed ? "removed line" : "line"} ${line}`;
  return (
    <Popover
      label={name}
      hint="Hover or click for how to ask the LLM about this line."
      className={className}
      content={(pinned, close) => (
        <>
          <span className="block">{hint}</span>
          {pinned && (
            <button
              type="button"
              // Keeps a selection of several lines for the click.
              onMouseDown={(event) => event.preventDefault()}
              onClick={(event) => {
                event.stopPropagation();
                onAsk(event.currentTarget);
                close();
              }}
              className="min-h-6 cursor-pointer justify-self-start rounded border border-border bg-surface px-2 text-text hover:border-accent"
            >
              Ask about this line
            </button>
          )}
        </>
      )}
    >
      <button
        type="button"
        data-ask-line
        aria-label={name}
        tabIndex={tabbable ? 0 : -1}
        onFocus={onFocus}
        // Keeps a selection of several lines for the click.
        onMouseDown={(event) => event.preventDefault()}
        className="inline-flex h-[1.35em] w-5 shrink-0 cursor-pointer items-center justify-center rounded border-0 bg-transparent p-0 font-sans text-[11px] font-bold text-muted opacity-0 group-hover/line:opacity-100 group-focus-within/line:opacity-100 hover:bg-accent-bg hover:text-accent focus-visible:opacity-100 aria-expanded:opacity-100 before:content-['?']"
      />
    </Popover>
  );
}

// Arrow keys, Home and End move between line actions (comment, or Ask): one tab stop for all lines.
export function moveLineFocus(event: KeyboardEvent<HTMLElement>) {
  const target = event.target as HTMLElement;
  const kind =
    "lineAction" in target.dataset
      ? "[data-line-action]"
      : "askLine" in target.dataset
        ? "[data-ask-line]"
        : null;
  if (!kind) return;
  const all = [...event.currentTarget.querySelectorAll<HTMLElement>(kind)];
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
 * Line renderer of one range (brief §4.3 Code view): green/red fill and +/− on changed lines (`CHANGE_LOOK`), a
 * remark bar in its own column (`REMARK_LOOK`; no hover tooltip: was → is is in the marker's popover), `≠` on differing moved lines, markers,
 * removed code inline, hidden comment/import lines folded in place.
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
  onAskLine,
  onLoadDeleted,
  onEditComment,
  onDeleteComment,
  onShowInInspector,
  otherThreadStep,
  bandOf,
  remarkOwner,
  selection,
  hunkOf,
  selectedHunk = null,
  onSelectHunk,
  removedAnnotations = [],
  remarkStartOnly = false,
  askLineHint,
}: CodeViewProps) {
  const dragging = useRef(false);
  useEffect(() => {
    const stop = () => {
      dragging.current = false;
    };
    window.addEventListener("pointerup", stop);
    return () => window.removeEventListener("pointerup", stop);
  }, []);
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
  // Hidden lines the reader showed in place (per view, not stored), by line key; the top-bar switch resets them.
  const [opened, setOpened] = useState<{ hide: boolean; lines: ReadonlySet<string> }>({
    hide: hideContextChanges,
    lines: new Set(),
  });
  if (opened.hide !== hideContextChanges) setOpened({ hide: hideContextChanges, lines: new Set() });
  const shownLines = opened.hide === hideContextChanges ? opened.lines : new Set<string>();
  const setShownLines = (update: (all: ReadonlySet<string>) => ReadonlySet<string>) =>
    setOpened((s) => ({ hide: s.hide, lines: update(s.lines) }));
  const isHidden = (ref: LineRef) =>
    (ref.side === lines.side ? hidden : hiddenBase).has(ref.line) && !shownLines.has(keyOf(ref));
  const hiddenRemoved = lines.deleted.reduce(
    (n, block) =>
      n + block.lines.filter((_, i) => isHidden({ side: "base", line: block.base_start + i })).length,
    0,
  );
  const hiddenCount =
    [...hidden].filter((line) => isHidden({ side: lines.side, line })).length + hiddenRemoved;
  const segments = buildSegments(lines, annotations, removedAnnotations);
  // A remark range continues across removed blocks inside it: one run, one label, one bar.
  type Run = { kind: "run"; parts: Segment[]; annotations: Annotation[] };
  const items: (Segment | Run)[] = [];
  {
    let run: Run | null = null;
    let between: Segment[] = [];
    const close = () => {
      if (run) items.push(run);
      items.push(...between);
      run = null;
      between = [];
    };
    for (const segment of segments) {
      const marked = segment.kind === "rows" ? [] : segment.annotations;
      const open = run as Run | null;
      if (marked.length > 0 && open && marked.some((a) => open.annotations.includes(a))) {
        open.parts.push(...between, segment);
        between = [];
        for (const a of marked) if (!open.annotations.includes(a)) open.annotations.push(a);
      } else if (marked.length > 0) {
        close();
        run = { kind: "run", parts: [segment], annotations: [...marked] };
      } else if (open && segment.kind === "deleted") {
        between.push(segment);
      } else {
        close();
        items.push(segment);
      }
    }
    close();
  }
  const baseRows = (block: DeletedBlock) =>
    block.lines.map((text, i) => ({ line: block.base_start + i, text, index: i }));
  const keys = segments.flatMap((s) =>
    (s.kind === "deleted"
      ? [
          ...baseRows(s.block).map((r) => ({ side: "base" as LineSide, line: r.line })),
          ...s.paired.map((row) => ({ side: lines.side, line: row.line })),
        ]
      : s.rows.map((r) => ({ side: lines.side, line: r.line }))
    )
      .filter((ref) => !isHidden(ref))
      .map(keyOf),
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
    onAskLine,
    onFocusLine: setFocusKey,
    onShowInInspector,
    unrated: band === null,
    otherThreadStep,
    remarkOwner,
    bandOf,
    selection:
      selection === undefined ? undefined : selection && { ...selection, side: selection.side ?? lines.side },
    hunkOf,
    selectedHunk,
    hunkEdges: edgesOf(keys, hunkOf),
    dragging,
    askLineHint,
    onSelectHunk,
  };
  for (const t of threads) {
    if (t.path !== lines.path || t.side === null || t.line === null) continue;
    const key = keyOf({ side: t.side, line: t.line });
    ctx.threads.set(key, [...(ctx.threads.get(key) ?? []), t]);
  }
  // One remark marker per remark, on its first displayed line (a line that never folds, see `hiddenLines`).
  const remarksAt = new Map<number, Annotation[]>();
  for (const a of annotations) {
    const at = Math.max(a.range.start_line, lines.from);
    if (at > lines.to || (remarkStartOnly && at !== a.range.start_line)) continue;
    remarksAt.set(at, [...(remarksAt.get(at) ?? []), a]);
  }
  // Remarks on removed code: on the first shown removed line they cover.
  const removedAt = new Map<number, Annotation[]>();
  for (const a of removedAnnotations) {
    const at = lines.deleted
      .flatMap((d) => d.lines.map((_, i) => d.base_start + i))
      .find((n) => a.range.start_line <= n && n <= a.range.end_line);
    if (at === undefined || (remarkStartOnly && at !== a.range.start_line)) continue;
    removedAt.set(at, [...(removedAt.get(at) ?? []), a]);
  }
  const firstOf = (run: Run) => {
    const part = run.parts[0]!;
    if (part.kind === "highlight") return part.rows[0];
    if (part.kind !== "deleted") return undefined;
    return part.paired[0];
  };
  // The gutter fits the most marker buttons any displayed line has (remarks, threads, own comments).
  const buttons = Math.max(
    0,
    ...keys.map(
      (k) =>
        ((k.startsWith(`${lines.side}:`) ? remarksAt : removedAt).get(Number(k.split(":")[1]))?.length ?? 0) +
        Number(ctx.threads.has(k)) +
        Number(byLine.has(k)),
    ),
  );

  const maxLine = Math.max(lines.to, ...lines.deleted.map((d) => d.base_start + d.lines.length));
  const base = lines.side === "base";
  const unchangedContext = kind === "context" && lines.changed.length === 0;
  const label = `${lines.path}, lines ${lines.from} to ${lines.to}${base ? ", removed code at the base commit" : ""}`;
  const line = (row: Row) => (
    <Line
      key={row.line}
      ctx={ctx}
      side={lines.side}
      line={row.line}
      text={row.text}
      tokens={tokens?.[row.index]}
      row={row}
      remarks={remarksAt.get(row.line)}
    />
  );
  // Lines in order, a fold row where rule-covered lines are hidden; a shown fold keeps its Hide row.
  const folded = <T extends { line: number; text: string }>(
    side: LineSide,
    rows: readonly T[],
    render: (row: T) => ReactNode,
  ) =>
    foldRuns(rows, (r) => (side === lines.side ? hidden : hiddenBase).has(r.line)).map((piece: Piece<T>) => {
      if (piece.kind === "item") return render(piece.item);
      const refs = piece.items.map((r) => keyOf({ side, line: r.line }));
      const shown = refs.every((k) => shownLines.has(k));
      return (
        <div key={`fold:${refs[0]}`} className="contents">
          <FoldRow
            count={piece.items.length}
            removed={side === "base" && !(lines.side === "base")}
            shown={shown}
            onToggle={() =>
              setShownLines((all) => {
                const next = new Set(all);
                for (const k of refs) {
                  if (shown) next.delete(k);
                  else next.add(k);
                }
                return next;
              })
            }
          />
          {shown && piece.items.map(render)}
        </div>
      );
    });
  const deletedRun = (segment: Extract<Segment, { kind: "deleted" }>, nested: boolean) => {
    const { block } = segment;
    const after = block.after_line;
    return (
      <DeletedRun
        key={`d${after}:${block.base_start}`}
        block={block}
        nested={nested}
        rows={folded("base", baseRows(block), (r) => (
          <Line
            key={r.index}
            ctx={ctx}
            side="base"
            line={r.line}
            text={r.text}
            tokens={fragmentTokens?.[1 + lines.deleted.indexOf(block)]?.[r.index]}
            removed
            remarks={removedAt.get(r.line)}
          />
        ))}
        paired={folded(lines.side, segment.paired, line)}
        anchor={after < lines.from ? `before line ${lines.from}` : `after line ${after}`}
        onLoadDeleted={onLoadDeleted}
      />
    );
  };

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
          "--gutter": `${Math.max(50, 30 + 28 * buttons + (onAskLine ? 22 : 0))}px`,
        } as CSSProperties
      }
      className={`@container min-w-0 max-w-full overflow-auto rounded-md border border-border bg-code-bg py-0.5 font-mono text-[12.5px] leading-[1.55] ${
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
      {items.map((item) => {
        if (item.kind === "run") {
          const first = firstOf(item);
          return (
            <HighlightRun
              key={`h${item.parts[0]!.kind === "deleted" ? `d${item.parts[0]!.block.base_start}` : first?.line}`}
              annotations={item.annotations}
              band={bandOf ? highest(item.annotations.map(bandOf)) : band}
              remarkOwner={remarkOwner}
              selected={item.annotations.some((a) => a.id === selectedAnnotation)}
              onSelect={onSelectAnnotation}
            >
              {item.parts.map((part) =>
                part.kind === "deleted" ? deletedRun(part, true) : folded(lines.side, part.rows, line),
              )}
            </HighlightRun>
          );
        }
        if (item.kind === "deleted") return deletedRun(item, false);
        return (
          <div key={`r${item.rows[0]!.line}`} data-segment className={`${SEGMENT} border-transparent`}>
            {folded(lines.side, item.rows, line)}
          </div>
        );
      })}
    </div>
  );
}
