import { REMARK_LOOK } from "../code-view/changeLook";
import { Tooltip } from "../tooltip/Tooltip";
import type { RemarkSummary } from "./remarks";

const WORD = { high: "HIGH", medium: "MEDIUM", low: "LOW" } as const;

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

interface RemarkPillProps {
  /** Rating and remarks (`summariseRemarks`); nothing is shown for null. */
  summary: RemarkSummary | null;
  /** One step, or a flow (its highest step rating). */
  scope: "step" | "flow";
  /** What it is about, in the name: "this step", "step 5", "flow 2". */
  subject: string;
  /** Text before the pill. */
  label?: string;
  /**
   * Whether the pill shows the rating; false next to a flow's own rating label (the flow's rating is its highest
   * step rating on the wire, so it is shown once): the pill shows the remarks only.
   */
  withRating?: boolean;
  /** Goes to the first remark (or the rated step). */
  onSelect: () => void;
}

/**
 * The LLM's rating of a step (or a flow's highest step rating) and the number of LLM remarks, as one LLM-styled
 * pill: two separate facts, never remark counts per severity. Nothing without either: never an all-clear.
 */
export function RemarkPill({ summary, scope, subject, label, withRating = true, onSelect }: RemarkPillProps) {
  if (!summary || (!withRating && summary.remarks === 0)) return null;
  const { rating, rated, remarks, handled } = summary;
  const look = REMARK_LOOK[withRating ? (rating ?? "none") : "none"];
  const done = remarks > 0 && handled === remarks;
  const ratingText =
    withRating &&
    rating &&
    `${look.label?.split(" ")[0] ?? ""} ${WORD[rating]}${scope === "flow" ? ` ×${rated}` : ""}`;
  const countText = remarks > 0 ? plural(remarks, "remark") : null;
  const ratingName = rating
    ? scope === "flow"
      ? `LLM rated ${plural(rated, "step")} in ${subject} ${WORD[rating]} (not checked), the highest rating there`
      : `LLM rated ${subject} ${WORD[rating]} (not checked)`
    : scope === "flow"
      ? `LLM rated no step in ${subject}`
      : `LLM did not rate ${subject}`;
  const name = withRating
    ? `${ratingName}${remarks > 0 ? ` · ${plural(remarks, "LLM remark")}` : ""}`
    : `${plural(remarks, "LLM remark")} in ${subject}`;
  const breakdown =
    scope === "flow"
      ? `Steps by LLM rating: ${summary.ratings.map((r) => `${r.rating ? WORD[r.rating] : "not rated"} ${r.steps}`).join(", ")}${withRating ? "" : " (the flow's rating is its highest step rating)"}. `
      : "";
  const action =
    remarks === 0
      ? "Click to open the step."
      : scope === "flow" && rating
        ? `Click to go to the first LLM remark on a step rated ${WORD[rating]}.`
        : "Click to go to its first LLM remark.";
  return (
    <span className="inline-flex flex-wrap items-center gap-1">
      {label && <span className="text-[12px] text-muted">{label}</span>}
      <Tooltip tip={`${breakdown}${action}${done ? ` All ${remarks} have your feedback.` : ""}`}>
        <button
          type="button"
          aria-label={name}
          onClick={(event) => {
            event.stopPropagation();
            onSelect();
          }}
          className={`inline-flex min-h-6 cursor-pointer items-center gap-1 rounded border-0 border-l-[3px] bg-llm-bg px-1.5 font-mono text-[11px] font-semibold whitespace-nowrap ${look.bar} ${
            done ? "text-muted" : look.text
          }`}
        >
          {ratingText}
          {ratingText && countText && <span aria-hidden> · </span>}
          {countText && <span className="font-normal">{countText}</span>}
        </button>
      </Tooltip>
    </span>
  );
}
