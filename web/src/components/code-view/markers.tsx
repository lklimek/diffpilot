import { AskRemarkButton } from "../ask/AskRemarkButton";
import { GithubCommentText } from "../comments/GithubCommentText";
import type { Annotation, AnnotationId, GithubThread } from "../../api/generated/types";
import { MARKER, SpeechBubble } from "../comment/LineComments";
import { GithubLink, isGithubUrl } from "../inspector/text";
import { Popover } from "../popover/Popover";
import { CodeText } from "../section/CodeText";
import { FLAG_LOOK, flagLevel } from "../flag/flags";
import { annotationName, rangeText } from "../section/format";
import { Tooltip } from "../tooltip/Tooltip";

/** A remark or GitHub thread a marker's popover can show in the Inspector. */
export type MarkerTarget = { kind: "annotation"; id: AnnotationId } | { kind: "thread"; id: string };

/**
 * A non-interactive gutter mark (`+`, `−`, `≠`): its text is read with the line. No tooltip: a trigger on every
 * line could not take keyboard focus without hundreds of tab stops; the `?` legend explains the marks.
 */
export function Mark({
  mark,
  glyph,
  text,
  className = "",
}: {
  mark: string;
  glyph: string;
  text: string;
  className?: string;
}) {
  return (
    <span data-mark={mark} className={`inline-flex leading-none ${className}`}>
      <span aria-hidden>{glyph}</span>
      <span className="sr-only">{text}</span>
    </span>
  );
}

/** Opens the Inspector at the item; `step` ("N.M") when the item belongs to another step, which it switches to. */
function ShowInInspector({
  name,
  step,
  onClick,
}: {
  name: string;
  step?: string | null;
  onClick: () => void;
}) {
  const text = step ? `Show in step ${step}` : "Show in Inspector";
  return (
    <Tooltip
      tip={
        step
          ? `Switches to step ${step}, where this item belongs, and highlights it there in the Inspector.`
          : "Opens the Inspector at this item and highlights it briefly."
      }
    >
      <button
        type="button"
        aria-label={`${text}: ${name}`}
        onClick={onClick}
        className="min-h-6 cursor-pointer border-0 bg-transparent p-0 text-left text-accent underline"
      >
        {text}
      </button>
    </Tooltip>
  );
}

const linesWord = (a: Annotation) =>
  `${a.range.start_line === a.range.end_line ? "line" : "lines"} ${rangeText(a.range)}`;

/**
 * `◆` of one LLM remark, on the remark's first displayed line (two remarks there, two markers): its popover holds
 * the remark's full text; `owner` names the remark's step when the code mixes steps ("flow 1 · step 2").
 */
export function RemarkMarker({
  annotation: a,
  line,
  owner,
  unrated = false,
  onShowInInspector,
  onAskRemark,
}: {
  annotation: Annotation;
  line: number;
  owner?: string | null;
  /** The remark carries no risk rating; the popover says so. */
  unrated?: boolean;
  onAskRemark?: (annotation: Annotation) => void;
  onShowInInspector?: (target: MarkerTarget) => void;
}) {
  const name = `LLM remark at line ${line}: ${annotationName(a)}${owner ? ` (${owner})` : ""}`;
  const title = `${annotationName(a)} · ${linesWord(a)}`;
  const level = flagLevel(a);
  return (
    <Popover
      label={name}
      tag="llm"
      hint="LLM remark. Hover or click for its text."
      content={(pinned, close) => (
        <>
          {unrated && <span className="block text-muted">Risk not rated by the LLM.</span>}
          <span className="block">
            <span className={`block font-semibold ${level ? FLAG_LOOK[level].text : "text-llm"}`}>
              {level && <span aria-hidden>⚑ </span>}
              {title}
              {owner && <span className="font-normal text-muted"> · {owner}</span>}
            </span>
            {a.type === "behaviour" ? (
              <>
                <span className="block">
                  <span className="font-semibold">Was:</span> <CodeText text={a.was} />
                </span>
                <span className="mt-1 block">
                  <span className="font-semibold">Is:</span> <CodeText text={a.is} />
                </span>
              </>
            ) : (
              <span className="block whitespace-pre-wrap">
                <CodeText text={a.text ?? ""} />
              </span>
            )}
            {a.flag && a.flag.scenario.length > 0 && (
              <span role="list" className="block pl-1">
                {a.flag.scenario.map((line, i) => (
                  <span role="listitem" key={i} className="block">
                    {i + 1}. <CodeText text={line} />
                  </span>
                ))}
              </span>
            )}{" "}
            <span className="text-muted">({a.basis})</span>
            {pinned && (onShowInInspector || onAskRemark) && (
              <span className="flex flex-wrap items-baseline gap-x-3">
                {onShowInInspector && (
                  <ShowInInspector
                    name={title}
                    onClick={() => onShowInInspector({ kind: "annotation", id: a.id })}
                  />
                )}
                {onAskRemark && (
                  <AskRemarkButton
                    onAsk={() => {
                      close();
                      onAskRemark(a);
                    }}
                  />
                )}
              </span>
            )}
          </span>
        </>
      )}
    >
      <button
        type="button"
        data-mark="remark"
        data-annotation={a.id}
        aria-label={name}
        className={`${MARKER} text-[15px] text-llm hover:text-text`}
      >
        <span aria-hidden>◆</span>
      </button>
    </Popover>
  );
}

/** Speech bubble in slate for the GitHub review threads of a line: author, excerpt and links in its popover. */
export function ThreadMarker({
  threads,
  line,
  onShowInInspector,
  otherStep,
}: {
  threads: readonly GithubThread[];
  line: number;
  onShowInInspector?: (target: MarkerTarget) => void;
  /** "N.M" of a thread's step when it is not the step shown. */
  otherStep?: (thread: GithubThread) => string | null;
}) {
  const n = threads.length;
  return (
    <Popover
      label={`${n === 1 ? "GitHub thread" : "GitHub threads"} at line ${line}`}
      tag="github"
      hint="GitHub review thread. Hover or click for its text."
      content={(pinned) =>
        threads.map((t) => (
          <span key={t.id} className="block">
            <span className="block font-semibold">@{t.author}</span>{" "}
            <span className="line-clamp-4 whitespace-pre-wrap">
              <GithubCommentText text={t.excerpt} />
            </span>
            {pinned && ((onShowInInspector && t.step_id !== null) || isGithubUrl(t.url)) && (
              <span className="mt-0.5 flex flex-wrap items-center gap-x-3">
                {onShowInInspector && t.step_id !== null && (
                  <ShowInInspector
                    name={`GitHub thread by ${t.author}`}
                    step={otherStep?.(t)}
                    onClick={() => onShowInInspector({ kind: "thread", id: t.id })}
                  />
                )}
                {isGithubUrl(t.url) && (
                  <GithubLink url={t.url} tip="Open the thread on GitHub (new tab)">
                    Open on GitHub
                  </GithubLink>
                )}
              </span>
            )}
          </span>
        ))
      }
    >
      <button
        type="button"
        data-mark="thread"
        aria-label={
          n === 1
            ? `GitHub thread by ${threads[0]!.author} at line ${line}`
            : `${n} GitHub threads at line ${line}`
        }
        className={`${MARKER} text-[10px] font-bold text-thread hover:text-text`}
      >
        <SpeechBubble />
        {n > 1 && (
          <span aria-hidden className="absolute right-0 bottom-0 rounded-sm bg-surface px-px">
            {n}
          </span>
        )}
      </button>
    </Popover>
  );
}
