import { useId, useLayoutEffect, useRef, useState } from "react";
import type { Annotation, Evidence, FlagLevel } from "../../api/generated/types";
import { AskRemarkButton } from "../ask/AskRemarkButton";
import { CodeText } from "../section/CodeText";
import { EvidenceLinks } from "../section/EvidenceLinks";
import { ProvenanceTag } from "../section/ProvenanceTag";
import { UseAsCommentButton } from "../comment/UseAsCommentButton";
import { Tooltip } from "../tooltip/Tooltip";
import { FLAG_LOOK, flagLevel } from "./flags";

/** The level of a flag as a word with the ⚑ glyph (never colour alone); its tooltip says it is an LLM statement. */
export function FlagMark({ level }: { level: FlagLevel }) {
  const look = FLAG_LOOK[level];
  return (
    <Tooltip tip={look.tip}>
      <span
        data-flag-level={level}
        tabIndex={0}
        className={`inline-block cursor-help rounded px-1.5 text-[11px] leading-[18px] font-semibold whitespace-nowrap ${look.bg} ${look.text}`}
      >
        {look.label}
      </span>
    </Tooltip>
  );
}

/**
 * Whether the element's content is taller than its box, re-measured when its size changes. Measured only while
 * `clamped`: an expanded list always fits, and re-measuring it would hide the toggle (and drop its focus).
 */
function useOverflow<T extends HTMLElement>(clamped: boolean) {
  const ref = useRef<T>(null);
  const [overflow, setOverflow] = useState(false);
  const measuring = useRef(clamped);
  useLayoutEffect(() => {
    measuring.current = clamped;
  }, [clamped]);
  useLayoutEffect(() => {
    const element = ref.current;
    if (!element) return;
    const measure = () => {
      if (measuring.current) setOverflow(element.scrollHeight > element.clientHeight + 1);
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  return [ref, overflow] as const;
}

interface FlagCardProps {
  /** A `flag` annotation. */
  annotation: Annotation;
  /** File of the step, for short evidence links. */
  currentFile?: string;
  onEvidence?: (evidence: Evidence) => void;
  /** Shows the flagged lines; the title becomes a link to them. */
  onOpen?: () => void;
  /** Opens the comment composer on the flagged lines with this flag as a draft; no button without it. */
  onUseAsComment?: () => void;
  onAskRemark?: () => void;
  /** The flag's step where flags of several steps are listed together ("step 5 of flow 1"). */
  owner?: string;
}

/**
 * One LLM flag: level, title, the scenario as a numbered list clipped to 5 lines with Show all / Show less,
 * what was not checked, and its evidence. A flag stored before levels shows its text as the title only.
 */
export function FlagCard({
  annotation,
  currentFile,
  onEvidence,
  onOpen,
  onUseAsComment,
  onAskRemark,
  owner,
}: FlagCardProps) {
  const level = flagLevel(annotation) ?? "unknown";
  const flag = annotation.flag;
  const scenario = flag?.scenario ?? [];
  const notChecked = flag?.not_checked.trim() ?? "";
  const listId = useId();
  const [expanded, setExpanded] = useState(false);
  const [list, overflow] = useOverflow<HTMLOListElement>(!expanded);
  const title = <CodeText text={annotation.text ?? ""} />;
  return (
    <li
      data-annotation-id={annotation.id}
      data-flag={level}
      className={`grid gap-1 rounded-md border border-l-4 border-border bg-surface-2 px-2.5 py-1.5 ${FLAG_LOOK[level].border}`}
    >
      <p className="flex flex-wrap items-center gap-1.5">
        <FlagMark level={level} />
        <ProvenanceTag source="llm" />
        {owner && <span className="text-[12px] text-muted">· {owner}</span>}
      </p>
      <p className="font-semibold">
        {onOpen ? (
          <Tooltip tip="Show the lines this flag is about.">
            <button type="button" onClick={onOpen} className="cursor-pointer text-left text-accent underline">
              {title}
            </button>
          </Tooltip>
        ) : (
          title
        )}
      </p>
      {scenario.length > 0 && (
        <div>
          <ol
            ref={list}
            id={listId}
            aria-label="Scenario"
            className={`m-0 list-decimal pl-5 ${expanded ? "" : "max-h-[5lh] overflow-hidden"}`}
          >
            {scenario.map((line, i) => (
              <li key={i}>
                <CodeText text={line} />
              </li>
            ))}
          </ol>
          {(overflow || expanded) && (
            <Tooltip
              tip={expanded ? "Clip the scenario to 5 lines again." : "Show every step of the scenario."}
            >
              <button
                type="button"
                aria-expanded={expanded}
                aria-controls={listId}
                onClick={() => setExpanded((open) => !open)}
                className="cursor-pointer text-[12px] text-accent underline"
              >
                {expanded ? "Show less" : "Show all"}
              </button>
            </Tooltip>
          )}
        </div>
      )}
      {notChecked && (
        <p className="text-muted">
          Not checked: <CodeText text={notChecked} />
        </p>
      )}
      {!flag?.scenario.length && level === "unknown" && (
        <p className="text-[11.5px] text-muted">
          Stored before flags had levels: the LLM asked this as a question, without a scenario.
        </p>
      )}
      <EvidenceLinks
        basis={annotation.basis}
        evidence={annotation.evidence}
        onEvidence={onEvidence}
        currentFile={currentFile}
      />
      {(onUseAsComment || onAskRemark) && (
        <div className="flex flex-wrap items-baseline gap-x-3">
          {onUseAsComment && <UseAsCommentButton onClick={onUseAsComment} what="this flag" />}
          {onAskRemark && <AskRemarkButton onAsk={onAskRemark} />}
        </div>
      )}
    </li>
  );
}
