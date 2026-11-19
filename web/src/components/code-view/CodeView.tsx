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
  /** Comment action on a displayed line; without it line numbers are plain text. */
  onLineAction?: (target: CommentTarget) => void;
  /** "Ask about this line" on a displayed line (the selected lines when the line is in a selection); none without it. */
  onAskLine?: (target: CommentTarget) => void;
  /** Load the rest of a deleted block whose `lines` were cut (`lines.length < total_lines`). */
  onLoadDeleted?: (block: DeletedBlock) => void;
  /** "Show in Inspector" of a pinned remark or thread popover; without it the link is not offered. */
  onShowInInspector?: (target: MarkerTarget) => void;
  /** "N.M" of a thread's step when it is not the step shown; its Show in Inspector then names that step. */
  otherThreadStep?: (thread: GithubThread) => string | null;
}

const BANDS = REMARK_LOOK;

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
  onAskLine?: (target: CommentTarget) => void;
  onFocusLine: (key: string) => void;
  onShowInInspector?: (target: MarkerTarget) => void;
  unrated: boolean;
  otherThreadStep?: (thread: GithubThread) => string | null;
}

interface LineProps {
  ctx: LineContext;
  side: LineSide;
  line: number;
  text: string;
  tokens: Token[] | undefined;
  row?: Row;
  removed?: boolean;
  /** First line of an LLM highlight: the remarks its marker shows. */
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
        className={`group/line grid min-w-full grid-cols-[var(--ln)_var(--gutter)_max-content] ${tall ? "min-h-6 items-center" : ""} ${background}`}
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
          {remarks && remarks.length > 0 && (
            <RemarkMarker
              annotations={remarks}
              line={line}
              unrated={ctx.unrated}
              onShowInInspector={ctx.onShowInInspector}
            />
          )}
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
  selected: boolean;
  onSelect?: (ids: AnnotationId[]) => void;
  /** The lines of the range, with removed blocks and fold rows inside it. */
  children: ReactNode;
}

/** One LLM remark range: a single label at its top and a bar along it; the lines keep their own look. */
function HighlightRun({ annotations, band, selected, onSelect, children }: HighlightRunProps) {
  const descriptionId = useId();
  const look = BANDS[band ?? "none"];
  const flag = strongestFlag(annotations);
  const ids = annotations.map((a) => a.id);
  const remark = `LLM remark: ${[...new Set(annotations.map((a) => a.type))].join(", ")}`;
  const text = [flag && FLAG_LOOK[flag].label, look.label, remark].filter(Boolean).join(" · ");
  const description = <WasIs annotations={annotations} unrated={band === null} />;
  const labelClass = `${STRIP} border-0 bg-transparent text-left text-[10.5px] leading-[17px] font-bold ${flag ? FLAG_LOOK[flag].text : look.text} ${
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
              content={(pinned, close) => (
                <>
                  <span className="block">
                    Shows the rest of the deleted lines of this block, from the base commit.
                  </span>
                  {pinned && (
                    <button
                      type="button"
                      onClick={() => {
                        onLoadDeleted(block);
                        close();
                      }}
                      className="min-h-6 cursor-pointer justify-self-start rounded border border-border bg-surface px-2 text-text hover:border-accent"
                    >
                      Show the rest
                    </button>
                  )}
                </>
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
  className = "",
}: {
  line: number;
  removed: boolean;
  tabbable: boolean;
  onFocus: () => void;
  onAsk: (button: HTMLElement) => void;
  className?: string;
}) {
  const name = `Ask the LLM about ${removed ? "removed line" : "line"} ${line}`;
  return (
    <Popover
      label={name}
      className={className}
      content={(pinned, close) => (
        <>
          <span className="block">
            Asks the LLM about this line (or the selected lines): puts the file and line into the Ask field,
            nothing is sent yet. The answer is LLM output.
          </span>
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
  const segments = buildSegments(lines, annotations);
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
  };
  for (const t of threads) {
    if (t.path !== lines.path || t.side === null || t.line === null) continue;
    const key = keyOf({ side: t.side, line: t.line });
    ctx.threads.set(key, [...(ctx.threads.get(key) ?? []), t]);
  }
  // The remark marker sits on the first line of each run its remarks cover (the line that never folds).
  const firstOf = (run: Run) => {
    const part = run.parts[0]!;
    if (part.kind === "highlight") return part.rows[0];
    if (part.kind !== "deleted") return undefined;
    const covered = (r: Row) =>
      run.annotations.some((a) => a.range.start_line <= r.line && r.line <= a.range.end_line);
    return part.paired.find(covered) ?? part.paired[0];
  };
  const remarkKeys = new Set(
    items.flatMap((item) => {
      const first = item.kind === "run" ? firstOf(item) : undefined;
      return first ? [keyOf({ side: lines.side, line: first.line })] : [];
    }),
  );
  // The gutter fits the most marker buttons any displayed line has (remark, threads, own comments).
  const buttons = Math.max(
    0,
    ...keys.map((k) => Number(remarkKeys.has(k)) + Number(ctx.threads.has(k)) + Number(byLine.has(k))),
  );

  const maxLine = Math.max(lines.to, ...lines.deleted.map((d) => d.base_start + d.lines.length));
  const base = lines.side === "base";
  const unchangedContext = kind === "context" && lines.changed.length === 0;
  const label = `${lines.path}, lines ${lines.from} to ${lines.to}${base ? ", removed code at the base commit" : ""}`;
  const line = (row: Row, remarks?: readonly Annotation[]) => (
    <Line
      key={row.line}
      ctx={ctx}
      side={lines.side}
      line={row.line}
      text={row.text}
      tokens={tokens?.[row.index]}
      row={row}
      remarks={remarks}
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
  const deletedRun = (
    segment: Extract<Segment, { kind: "deleted" }>,
    nested: boolean,
    first?: Row,
    remarks?: readonly Annotation[],
  ) => {
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
          />
        ))}
        paired={folded(lines.side, segment.paired, (row) => line(row, row === first ? remarks : undefined))}
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
      {items.map((item) => {
        if (item.kind === "run") {
          const first = firstOf(item);
          return (
            <HighlightRun
              key={`h${item.parts[0]!.kind === "deleted" ? `d${item.parts[0]!.block.base_start}` : first?.line}`}
              annotations={item.annotations}
              band={band}
              selected={item.annotations.some((a) => a.id === selectedAnnotation)}
              onSelect={onSelectAnnotation}
            >
              {item.parts.map((part) =>
                part.kind === "deleted"
                  ? deletedRun(part, true, first, item.annotations)
                  : folded(lines.side, part.rows, (row) =>
                      line(row, row === first ? item.annotations : undefined),
                    ),
              )}
            </HighlightRun>
          );
        }
        if (item.kind === "deleted") return deletedRun(item, false);
        return (
          <div key={`r${item.rows[0]!.line}`} data-segment className={`${SEGMENT} border-transparent`}>
            {folded(lines.side, item.rows, (row) => line(row))}
          </div>
        );
      })}
    </div>
  );
}
