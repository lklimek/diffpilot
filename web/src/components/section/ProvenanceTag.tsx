import { Tooltip } from "../tooltip/Tooltip";

/** Where a piece of content comes from: written by the LLM, or taken from GitHub. */
export type ProvenanceSource = "llm" | "github";

// Says where text comes from, never that it is correct: not a fact badge (brief §1).
const TAG: Record<ProvenanceSource, { text: string; tip: string; look: string }> = {
  llm: {
    text: "LLM",
    tip: "This text was written by the LLM; it was not checked.",
    look: "bg-llm-bg text-llm",
  },
  github: {
    text: "GitHub",
    tip: "Taken from GitHub as written there.",
    look: "border border-border text-muted",
  },
};

/** Small tag naming the source of the content next to it. */
export function ProvenanceTag({ source }: { source: ProvenanceSource }) {
  const { text, tip, look } = TAG[source];
  return (
    <Tooltip tip={tip}>
      <span
        data-provenance={source}
        tabIndex={0}
        className={`inline-block cursor-help rounded px-1 align-baseline text-[10.5px] leading-4 font-semibold whitespace-nowrap ${look}`}
      >
        {text}
      </span>
    </Tooltip>
  );
}
