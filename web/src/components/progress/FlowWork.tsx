import { Tooltip } from "../tooltip/Tooltip";
import { AnalyseAllButton } from "./AnalyseAllButton";

interface Props {
  done: number;
  total: number;
  /** Changed lines in the planned ranges of the flows not analysed yet. */
  remainingLines: number;
  /** `background_analysis` of the analysis. */
  background: boolean;
  /** No flow is queued or being analysed and nothing will start by itself. */
  parked: boolean;
  busy: boolean;
  progressHref: string;
  error?: string;
  onBackground: (enabled: boolean) => void;
}

const PARKED = "Nothing is being analysed. Choose a flow to analyse, or analyse all remaining flows.";

/** Analysis work, separate from the reviewer's ticks: flows run on request, or all with one opt-in. */
export function FlowWork({
  done,
  total,
  remainingLines,
  background,
  parked,
  busy,
  progressHref,
  error,
  onBackground,
}: Props) {
  const remaining = total - done;
  return (
    <section
      aria-label="Analysis work"
      className="mb-3 grid gap-1.5 rounded border border-border bg-surface px-3 py-2 text-[12.5px]"
    >
      <p>
        {done} of {total} flows analysed. Pending flows have not been assessed.
      </p>
      {parked && <p role="status">{PARKED}</p>}
      {(remaining > 0 || background) && (
        <p>
          <AnalyseAllButton
            enabled={background}
            busy={busy}
            hint={`${remaining} ${remaining === 1 ? "flow" : "flows"} not analysed yet, ${remainingLines} changed lines.`}
            onChange={onBackground}
          />
          {remaining > 0 && (
            <span className="ml-2 text-xs text-muted">
              {remaining} not analysed · {remainingLines} changed lines
            </span>
          )}
        </p>
      )}
      <p>
        <Tooltip tip="Shows analysis timing and the controls to pause or cancel further work.">
          <a href={progressHref}>Analysis progress, pause and cancel</a>
        </Tooltip>
      </p>
      {error && <p role="alert">{error}</p>}
    </section>
  );
}
