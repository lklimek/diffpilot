import type { Annotation, AnnotationId, GithubThread } from "../../api/generated/types";
import { MARKER, SpeechBubble } from "../comment/LineComments";
import { GithubLink, isGithubUrl } from "../inspector/text";
import { Popover } from "../popover/Popover";
import { CodeText } from "../section/CodeText";
import { ANNOTATION, rangeText } from "../section/format";
import { Tooltip } from "../tooltip/Tooltip";

/** A remark or GitHub thread a marker's popover can show in the Inspector. */
export type MarkerTarget = { kind: "annotation"; id: AnnotationId } | { kind: "thread"; id: string };

/** A non-interactive gutter mark (`−`, `≠`) with a project tooltip; its text is also read with the line. */
export function Mark({
  mark,
  glyph,
  text,
  tip,
  className = "",
}: {
  mark: string;
  glyph: string;
  text: string;
  tip: string;
  className?: string;
}) {
  return (
    <Tooltip tip={tip} className="inline-flex">
      <span data-mark={mark} className={`cursor-help leading-none ${className}`}>
        <span aria-hidden>{glyph}</span>
        <span className="sr-only">{text}</span>
      </span>
    </Tooltip>
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

/** `◆` on the first line of an LLM highlight: its popover holds the full text of the highlight's remarks. */
export function RemarkMarker({
  annotations,
  line,
  onShowInInspector,
}: {
  annotations: readonly Annotation[];
  line: number;
  onShowInInspector?: (target: MarkerTarget) => void;
}) {
  const n = annotations.length;
  return (
    <Popover
      label={`LLM remarks at line ${line}`}
      tag="llm"
      content={(pinned) =>
        annotations.map((a) => {
          const title = `${ANNOTATION[a.type]} · ${linesWord(a)}`;
          return (
            <span key={a.id} className="block">
              <span className="block font-semibold text-llm">{title}</span>
              {a.type === "behaviour" ? (
                <>
                  <span className="block">Was: {a.was}</span> <span className="block">Is: {a.is}</span>
                </>
              ) : (
                <span className="block whitespace-pre-wrap">
                  <CodeText text={a.text ?? ""} />
                </span>
              )}{" "}
              <span className="text-muted">({a.basis})</span>
              {pinned && onShowInInspector && (
                <span className="block">
                  <ShowInInspector
                    name={title}
                    onClick={() => onShowInInspector({ kind: "annotation", id: a.id })}
                  />
                </span>
              )}
            </span>
          );
        })
      }
    >
      <button
        type="button"
        data-mark="remark"
        aria-label={`${n === 1 ? "LLM remark" : `${n} LLM remarks`} at line ${line}`}
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
      content={(pinned) =>
        threads.map((t) => (
          <span key={t.id} className="block">
            <span className="block font-semibold">@{t.author}</span>{" "}
            <span className="line-clamp-4 whitespace-pre-wrap">{t.excerpt}</span>
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
