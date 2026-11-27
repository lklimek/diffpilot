import type { ReactNode } from "react";
import { ProvenanceTag } from "../../section/ProvenanceTag";
import { CodeText } from "../../section/CodeText";

interface LlmLineProps {
  /** LLM-written text, with verified file mentions when an analysis is available. */
  text: string;
  /** Small caps label before the tag, e.g. `Next`. */
  label?: string;
  /** Marker after the tag, e.g. the provisional pill of an inherited judgement. */
  marker?: ReactNode;
  judgementDate?: string | null;
  className?: string;
}

/** One line of LLM-written text, always with the "LLM" tag (ADR-016: every LLM text carries it). */
export function LlmLine({ text, label, marker, judgementDate, className = "" }: LlmLineProps) {
  return (
    <div data-llm-text className={`m-0 text-[12.5px] ${className}`}>
      {label && (
        <span className="mr-1 text-[11px] font-bold tracking-wide text-muted uppercase">{label}</span>
      )}
      {marker}
      {marker ? " " : ""}
      <ProvenanceTag source="llm" detail={judgementDate ? `Judged on ${judgementDate}.` : undefined} />{" "}
      <CodeText text={text} />
    </div>
  );
}
