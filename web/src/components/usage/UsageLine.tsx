import type { UsageTotals } from "../../api/generated/types";
import { Tooltip } from "../tooltip/Tooltip";

const number = new Intl.NumberFormat("en", { maximumFractionDigits: 1 });
const scale = (value: number) => (value >= 1_000_000 ? 1_000_000 : value >= 1_000 ? 1_000 : 1);
const compact = (value: number, unit = scale(value)) =>
  `${number.format(value / unit)}${unit === 1_000_000 ? " M" : unit === 1_000 ? " k" : ""}`;

/** Provider reports, not a bill or a confidence score; cached input is already included in input. */
export function UsageLine({
  usage,
  label = "LLM usage so far",
}: {
  usage?: UsageTotals | null;
  label?: string;
}) {
  if (!usage?.jobs) return null;
  const text = `${label}: ${compact(usage.input_tokens)} input (${compact(usage.cached_input_tokens, scale(usage.input_tokens))} cached), ${compact(usage.output_tokens)} output tokens in ${usage.jobs} ${usage.jobs === 1 ? "job" : "jobs"}`;
  return (
    <p className="text-xs text-muted">
      <Tooltip tip="Reported by the provider for completed jobs, including Ask. Cached input is included in input. Reasoning tokens are not available separately.">
        <span tabIndex={0}>{text}</span>
      </Tooltip>
    </p>
  );
}
