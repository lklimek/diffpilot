import { useId, type ReactNode } from "react";
import type { SectionControl } from "./sectionState";

interface CollapsibleProps {
  /** Key of the remembered open / collapsed state. */
  sectionKey: string;
  /** Heading; also the accessible name of the region. */
  title: string;
  /** `llm` for LLM content, `facts` for deterministic or GitHub facts (brief §1: visibly distinct). */
  tone: "llm" | "facts";
  /** Where facts come from, e.g. "From the diff; no LLM."; shown under the heading. */
  source?: string;
  sections: SectionControl;
  children: ReactNode;
}

/** A collapsible inspector section (disclosure button + region); its state is shared and remembered. */
export function Collapsible({ sectionKey, title, tone, source, sections, children }: CollapsibleProps) {
  const id = useId();
  const open = sections.isOpen(sectionKey);
  const look = tone === "llm" ? "border-l-llm" : "border-l-mv-bd";
  return (
    <section
      aria-labelledby={`${id}-h`}
      data-tone={tone}
      data-section={sectionKey}
      className={`min-w-0 border-b border-l-4 border-b-border ${look} px-3 py-1.5`}
    >
      <h3 id={`${id}-h`} className="m-0 text-[13px]">
        <button
          type="button"
          aria-expanded={open}
          aria-controls={`${id}-b`}
          onClick={() => sections.toggle(sectionKey)}
          className={`flex w-full cursor-pointer items-baseline gap-1.5 rounded py-0.5 text-left font-bold before:inline-block before:w-3 before:text-muted ${open ? "before:content-['▾']" : "before:content-['▸']"} ${tone === "llm" ? "text-llm" : "text-text"}`}
        >
          {title}
        </button>
      </h3>
      <div id={`${id}-b`} hidden={!open} className="pb-1.5 pl-[18px] text-[12.5px]">
        {source && <p className="mb-1 text-[11.5px] text-muted">{source}</p>}
        {children}
      </div>
    </section>
  );
}
