import type { Annotation, FlagLevel } from "../../api/generated/types";
import { Tooltip } from "../tooltip/Tooltip";
import { FLAG_LOOK, FLAG_WORD, countFlags, flagCountText, strongestFlag } from "./flags";

// The level is written next to the glyph: its colour is never the only signal.
const TEXT: Record<FlagLevel, string> = { red: "⚑ red", yellow: "⚑ yellow", unknown: "⚑" };

interface FlagMarkerProps {
  /** Remarks of the step or flow; only flags count. */
  annotations: readonly Annotation[];
  /** One step, or a flow (all its steps). */
  scope: "step" | "flow";
  /** What it is about, in the name: "step 5", "flow 2", "this step". */
  subject: string;
  /** Goes to the first flag at the strongest level. */
  onSelect: () => void;
}

/**
 * The strongest LLM flag of a step or flow, written in its level's colour; a click goes to the first flag at that
 * level, the tooltip counts the flags by level. Not a rating: it stands apart from the rating pill. Nothing
 * without flags (never an all-clear).
 */
export function FlagMarker({ annotations, scope, subject, onSelect }: FlagMarkerProps) {
  const level = strongestFlag(annotations);
  if (!level) return null;
  const look = FLAG_LOOK[level];
  const where = `${scope === "flow" ? "in" : "on"} ${subject}`;
  return (
    <Tooltip
      tip={`LLM flags ${where}: ${flagCountText(countFlags(annotations))}. Click to go to the first ${FLAG_WORD[level]}. LLM statements, not checked; the Inspector shows each flag with its scenario.`}
    >
      <button
        type="button"
        aria-label={`First ${FLAG_WORD[level]} ${where}`}
        onClick={(event) => {
          event.stopPropagation();
          onSelect();
        }}
        className={`inline-flex min-h-6 cursor-pointer items-center rounded border-0 px-1 font-mono text-[11px] font-semibold whitespace-nowrap ${look.bg} ${look.text}`}
      >
        {TEXT[level]}
      </button>
    </Tooltip>
  );
}
