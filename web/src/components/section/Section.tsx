import { useId, type ReactNode } from "react";

interface SectionProps {
  /** Heading; also the accessible name of the region ("LLM analysis" or "Facts"). */
  title: string;
  subtitle?: string;
  /** `llm` for LLM content, `facts` for deterministic facts: they never share a section (brief §1). */
  tone?: "llm" | "facts";
  children: ReactNode;
}

/** A titled region of a block; LLM content and deterministic facts are visibly separate. */
export function Section({ title, subtitle, tone = "facts", children }: SectionProps) {
  const id = useId();
  const look = tone === "llm" ? "border-l-llm" : "border-l-mv-bd";
  return (
    <section
      aria-labelledby={id}
      className={`min-w-0 rounded-lg border border-l-4 border-border ${look} bg-surface px-4 py-3`}
    >
      <div className="mb-1 flex flex-wrap items-baseline gap-x-2">
        <h3 id={id} className={`text-[13px] font-bold ${tone === "llm" ? "text-llm" : "text-text"}`}>
          {title}
        </h3>
        {subtitle && <p className="text-[12px] text-muted">{subtitle}</p>}
      </div>
      {children}
    </section>
  );
}

/** The one line per LLM panel required by brief §1. */
export function LlmLine() {
  return <p className="mb-1.5 text-[11.5px] text-llm">LLM analysis unless marked otherwise.</p>;
}

/** Small uppercase heading inside a section. */
export function Subhead({ children }: { children: ReactNode }) {
  return (
    <h4 className="mt-2.5 mb-1 text-[10.5px] font-bold tracking-wide text-muted uppercase">{children}</h4>
  );
}
