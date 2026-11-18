import type { RiskBand } from "../../api/generated/types";
import { REMARK_LOOK } from "../code-view/changeLook";
import { Tooltip } from "../tooltip/Tooltip";
import type { RemarkCount } from "./remarks";

const SHORT: Record<RiskBand, string> = { high: "HIGH", medium: "MED", low: "LOW" };

interface RemarkPillsProps {
  /** Remarks by band, highest first (`remarkCounts`). */
  counts: RemarkCount[];
  /** Where the remarks are, for the tooltip: "on this step", "in this flow". */
  subject: string;
  /** Text before the pills, e.g. "Remarks in this flow:". */
  label?: string;
  /** Accessible name of the group; defaults to "LLM remarks <subject>". */
  name?: string;
  /** Goes to the first remark of the band. */
  onSelect: (band: RiskBand) => void;
}

/**
 * LLM remarks as small pills by band (the LLM's rating, styled as LLM content, never a deterministic badge).
 * Nothing at all without remarks: an absence of remarks is never shown as an all-clear.
 */
export function RemarkPills({ counts, subject, label, name, onSelect }: RemarkPillsProps) {
  if (counts.length === 0) return null;
  const summary = counts.map((c) => `${c.count} ${c.band}`).join(", ");
  return (
    <span
      role="group"
      aria-label={name ?? `LLM remarks ${subject}`}
      className="inline-flex flex-wrap items-center gap-1"
    >
      {label && <span className="text-[12px] text-muted">{label}</span>}
      {counts.map(({ band, count, handled }) => {
        const look = REMARK_LOOK[band];
        const done = handled === count;
        const glyph = look.label?.split(" ")[0] ?? "";
        return (
          <Tooltip
            key={band}
            tip={`LLM remarks ${subject}: ${summary} — click to go to the first ${band} one${done ? `; all ${count} have your feedback` : ""}.`}
          >
            <button
              type="button"
              onClick={(event) => {
                event.stopPropagation();
                onSelect(band);
              }}
              className={`inline-flex min-h-6 cursor-pointer items-center rounded border-0 border-l-[3px] bg-llm-bg px-1.5 font-mono text-[11px] font-semibold whitespace-nowrap ${look.bar} ${
                done ? "text-muted" : look.text
              }`}
            >
              {`${glyph} ${SHORT[band]} ${count}`}
            </button>
          </Tooltip>
        );
      })}
    </span>
  );
}
