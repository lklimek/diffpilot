import { busyTip, buttonClass } from "../button/button";
import { Tooltip } from "../tooltip/Tooltip";

interface Props {
  /** The analysis's `background_analysis` switch. */
  enabled: boolean;
  busy: boolean;
  /** Extra sentence of the tip, e.g. what remains. */
  hint?: string;
  onChange: (enabled: boolean) => void;
}

/** The opt-in to analyse every remaining flow (`background_analysis`); otherwise flows run on request only. */
export function AnalyseAllButton({ enabled, busy, hint, onChange }: Props) {
  const tip = enabled
    ? "Stops queueing the remaining flows; the running job finishes. Flows you request still run."
    : "Queues every flow not analysed yet, by plan rank; you can stop it again. Flows you request run first.";
  return (
    <Tooltip tip={busyTip(hint ? `${tip} ${hint}` : tip, busy)}>
      <button
        type="button"
        aria-disabled={busy || undefined}
        onClick={() => !busy && onChange(!enabled)}
        className={buttonClass(enabled ? "plain" : "primary", true)}
      >
        {/* Wraps in a narrow Flows column instead of widening it. */}
        <span className="whitespace-normal">
          {enabled ? "Analysing all remaining flows — Stop" : "Analyse all remaining flows"}
        </span>
      </button>
    </Tooltip>
  );
}
