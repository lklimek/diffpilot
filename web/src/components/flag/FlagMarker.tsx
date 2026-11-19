import type { Annotation, FlagLevel } from "../../api/generated/types";
import { Tooltip } from "../tooltip/Tooltip";
import { FLAG_LOOK, countFlags, flagCountText, strongestFlag } from "./flags";

// The level is written next to the glyph: its colour is never the only signal.
const TEXT: Record<FlagLevel, string> = { red: "⚑ red", yellow: "⚑ yellow", unknown: "⚑" };

interface FlagMarkerProps {
  /** Remarks of the step or flow; only flags count. */
  annotations: readonly Annotation[];
  /** One step, or a flow (all its steps). */
  scope: "step" | "flow";
  /** What it is about, in the name: "step 5", "flow 2", "this step". */
  subject: string;
}

/**
 * The strongest LLM flag of a step or flow, written in its level's colour; its name counts the flags by level.
 * Not a rating: it stands apart from the rating pill. Nothing without flags (never an all-clear).
 */
export function FlagMarker({ annotations, scope, subject }: FlagMarkerProps) {
  const level = strongestFlag(annotations);
  if (!level) return null;
  const look = FLAG_LOOK[level];
  const name = `LLM flags ${scope === "flow" ? "in" : "on"} ${subject}: ${flagCountText(countFlags(annotations))}`;
  return (
    <Tooltip
      tip={`${name}. LLM statements, not checked; the Inspector shows each flag with its scenario on its step.`}
    >
      <span
        role="img"
        aria-label={name}
        tabIndex={0}
        className={`inline-flex min-h-6 cursor-help items-center rounded px-1 font-mono text-[11px] font-semibold whitespace-nowrap ${look.bg} ${look.text}`}
      >
        {TEXT[level]}
      </span>
    </Tooltip>
  );
}
