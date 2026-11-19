import type { ProvenanceBadge as Provenance } from "../../api/generated/types";
import { Tooltip } from "../tooltip/Tooltip";

// Deterministic badges only (brief §1): a badge names exactly what was checked; no badge means LLM analysis.
const PROVENANCE: Record<Provenance, { text: string; tip: string; tick: boolean }> = {
  text_identical: {
    text: "text identical",
    tip: "Deterministic check: the moved block is equal to the original after whitespace normalisation. Identical text does not mean unchanged behaviour.",
    tick: true,
  },
  classified_by_path: {
    text: "classified by path",
    tip: "Deterministic rule: the file path matched a built-in path rule. The content was not analysed.",
    tick: false,
  },
  classified_by_line_rule: {
    text: "classified by line rule",
    tip: "Deterministic rule: the lines matched a built-in line rule. The content was not analysed.",
    tick: false,
  },
  binary_content: {
    text: "binary file",
    tip: "Git treats this file as binary; it is not shown or analysed.",
    tick: false,
  },
};

const BASE =
  "inline-block cursor-help rounded border px-1.5 text-[11px] leading-[18px] font-semibold whitespace-nowrap";

/** Badge of a deterministic provenance check; the green tick only for the content check. */
export function ProvenanceBadge({ badge }: { badge: Provenance }) {
  const { text, tip, tick } = PROVENANCE[badge];
  const look = tick ? "border-add-bd bg-ok-bg text-ok" : "border-mv-bd bg-surface-2 text-text";
  return (
    <Tooltip tip={tip}>
      <span data-badge={badge} tabIndex={0} className={`${BASE} ${look}`}>
        {tick ? `✓ ${text}` : text}
      </span>
    </Tooltip>
  );
}

/** Badge of a moved block that is not identical: how many lines differ from the original (deterministic). */
export function DifferingLinesBadge({ count }: { count: number }) {
  return (
    <Tooltip tip="Deterministic check: lines of the moved block that differ from the original after whitespace normalisation.">
      <span data-badge="differing_lines" tabIndex={0} className={`${BASE} border-mv-bd bg-mv-bg text-mv`}>
        {`≠ ${count} ${count === 1 ? "line differs" : "lines differ"}`}
      </span>
    </Tooltip>
  );
}
