import { useId, useState } from "react";
import type { LlmSelection, ModelChoice, ReasoningEffort } from "../../api/generated/types";
import { SettingsCard } from "./Card";
import { Tooltip } from "../tooltip/Tooltip";
import { buttonClass } from "../button/button";
import { dateTimeText } from "../format/time";

/** One installation choice, passed in by Settings; saving never starts an analysis. */
export function LlmSelectionCard({
  selection,
  busy,
  saved,
  error,
  onSave,
}: {
  selection: LlmSelection;
  busy: boolean;
  saved?: boolean;
  error?: string;
  onSave: (choice: ModelChoice) => void;
}) {
  const id = useId();
  const [choice, setChoice] = useState(selection.current);
  return (
    <SettingsCard title="LLM model">
      <label htmlFor={`${id}-model`}>Model</label>
      <Tooltip
        tip={
          busy
            ? "Saving; the choice cannot be changed yet."
            : "The model used by new jobs across this installation."
        }
      >
        <select
          id={`${id}-model`}
          value={choice.model}
          aria-disabled={busy || undefined}
          onChange={(e) => !busy && setChoice({ ...choice, model: e.target.value })}
          className="w-full min-w-0 rounded border border-border bg-surface px-2 py-1"
        >
          {selection.models.map((model) => (
            <option key={model.id} value={model.id}>
              {model.label}
            </option>
          ))}
        </select>
      </Tooltip>
      <label htmlFor={`${id}-effort`}>Reasoning effort</label>
      <Tooltip
        tip={
          busy
            ? "Saving; the choice cannot be changed yet."
            : "How much reasoning the model is asked to do. Higher effort can take longer."
        }
      >
        <select
          id={`${id}-effort`}
          value={choice.reasoning_effort}
          aria-disabled={busy || undefined}
          onChange={(e) =>
            !busy && setChoice({ ...choice, reasoning_effort: e.target.value as ReasoningEffort })
          }
          className="w-full rounded border border-border bg-surface px-2 py-1"
        >
          {selection.efforts.map((effort) => (
            <option key={effort} value={effort}>
              {effort}
            </option>
          ))}
        </select>
      </Tooltip>
      <Tooltip
        tip={
          busy
            ? "Saving the installation choice. Wait for it to finish."
            : "Saves the choice for everyone using this installation."
        }
      >
        <button
          type="button"
          aria-disabled={busy || undefined}
          className={`${buttonClass("primary")} w-fit`}
          onClick={() => !busy && onSave(choice)}
        >
          Save
        </button>
      </Tooltip>
      <p className="text-xs text-muted">
        Applies to jobs started from now on, including the remaining jobs of running analyses. A job already
        submitted keeps its choice.
      </p>
      {selection.changed_by && selection.changed_at && (
        <p className="text-xs text-muted">
          changed by {selection.changed_by} at {dateTimeText(selection.changed_at)}
        </p>
      )}
      {saved && <p role="status">LLM choice saved.</p>}
      {error && <p role="alert">The LLM choice was not saved: {error}</p>}
    </SettingsCard>
  );
}
