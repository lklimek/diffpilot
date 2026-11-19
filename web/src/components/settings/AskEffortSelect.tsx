import { useId } from "react";
import type { ReasoningEffort } from "../../api/generated/types";
import { busyTip } from "../button/button";
import { Tooltip } from "../tooltip/Tooltip";
import { HelpTip } from "./HelpTip";

interface AskEffortSelectProps {
  /** Current Ask effort; absent = the default, medium. */
  effort?: ReasoningEffort;
  /** Efforts the server offers. */
  efforts: ReasoningEffort[];
  /** Saving: changes are ignored; the select keeps focus (`aria-disabled`). */
  busy: boolean;
  saved?: boolean;
  error?: string;
  onChange: (effort: ReasoningEffort) => void;
}

/** Installation-wide reasoning effort of Ask answers, inside the LLM card. */
export function AskEffortSelect({ effort, efforts, busy, saved, error, onChange }: AskEffortSelectProps) {
  const id = useId();
  return (
    <div className="mt-1 grid gap-2 border-t border-border pt-3">
      <span className="flex items-center gap-2">
        <label htmlFor={id}>Ask effort</label>
        <HelpTip topic="ask_effort" />
      </span>
      <Tooltip
        tip={busyTip(
          "How much reasoning the LLM does when it answers a question about a step. Lower effort answers sooner.",
          busy,
        )}
      >
        <select
          id={id}
          value={effort ?? "medium"}
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
        Applies to everyone using this installation. Saved when changed; questions asked from now on use it.
        The analysis keeps the reasoning effort above.
      </p>
      {saved && <p role="status">Ask effort saved.</p>}
      {error && <p role="alert">The Ask effort was not saved: {error}</p>}
    </div>
  );
}
