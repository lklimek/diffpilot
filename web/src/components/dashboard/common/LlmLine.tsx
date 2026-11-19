import type { ReactNode } from "react";
import { ProvenanceTag } from "../../section/ProvenanceTag";

interface LlmLineProps {
  /** LLM-written text (rendered as text only). */
  text: string;
  /** Small caps label before the tag, e.g. `Next`. */
  label?: string;
  /** Marker after the tag, e.g. the provisional pill of an inherited judgement. */
  marker?: ReactNode;
  className?: string;
}

/** One line of LLM-written text, always with the "LLM" tag (ADR-016: every LLM text carries it). */
export function LlmLine({ text, label, marker, className = "" }: LlmLineProps) {
  return (
    <p data-llm-text className={`m-0 text-[12.5px] ${className}`}>
      {label && (
        <span className="mr-1 text-[11px] font-bold tracking-wide text-muted uppercase">{label}</span>
      )}
      <ProvenanceTag source="llm" /> {marker}
      {marker ? " " : ""}
      {text}
    </p>
  );
}
