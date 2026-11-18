import { useId } from "react";
import type { ReasoningEffort } from "../../../api/generated/types";
import { busyTip } from "../../button/button";
import { Tooltip } from "../../tooltip/Tooltip";

interface JudgeEffortSelectProps {
  /** Current ranking effort; absent = the default, low. */
  effort?: ReasoningEffort;
  /** Efforts the server offers. */
  efforts: ReasoningEffort[];
  /** Saving: changes are ignored; the select keeps focus (`aria-disabled`). */
  busy: boolean;
  saved?: boolean;
  error?: string;
  onChange: (effort: ReasoningEffort) => void;
}

/** Installation-wide reasoning effort of the dashboard ranking (plan §F.6 G13), inside the LLM card. */
export function JudgeEffortSelect({ effort, efforts, busy, saved, error, onChange }: JudgeEffortSelectProps) {
  const id = useId();
  return (
    <div className="mt-1 grid gap-2 border-t border-border pt-3">
      <label htmlFor={id}>Dashboard ranking effort</label>
      <Tooltip
        tip={busyTip(
          "How much reasoning the LLM does when it ranks My work items. Higher effort can take longer.",
          busy,
        )}
      >
        <select
          id={id}
          value={effort ?? "low"}
          aria-disabled={busy || undefined}
          aria-describedby={`${id}-help`}
          onChange={(e) => !busy && onChange(e.target.value as ReasoningEffort)}
          className="w-full rounded border border-border bg-surface px-2 py-1"
        >
          {efforts.map((value) => (
            <option key={value} value={value}>
              {value}
            </option>
          ))}
        </select>
      </Tooltip>
      <p id={`${id}-help`} className="text-xs text-muted">
        Applies to everyone using this installation. Saved when changed; rankings started from now on use it.
      </p>
      {saved && <p role="status">Ranking effort saved.</p>}
      {error && <p role="alert">The ranking effort was not saved: {error}</p>}
    </div>
  );
}
