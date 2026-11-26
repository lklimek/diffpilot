import { Tooltip } from "../tooltip/Tooltip";
import { useEffect, useId, useState } from "react";
import type { LlmSelection, ModelChoice, ReasoningEffort } from "../../api/generated/types";
import { Section } from "./Section";
import { FormRow } from "./FormRow";
import { buttonClass } from "../button/button";
import { dateTimeText } from "../format/time";
import { useAction } from "../inspector/action";
import { HelpTip } from "./HelpTip";

export interface LlmChoiceChanges {
  model?: ModelChoice;
  ask?: ReasoningEffort;
  ranking?: ReasoningEffort;
  slots?: number;
}

type Draft = Partial<ModelChoice> & Pick<LlmChoiceChanges, "ask" | "ranking"> & { slots?: string };
const MAX_JOB_SLOTS = 65_535;

/** The payer's model and efforts, saved together. */
export function LlmSelectionCard({
  selection,
  jobSlots,
  onSave,
  onDirtyChange,
}: {
  selection: LlmSelection;
  jobSlots?: number;
  onSave: (changes: LlmChoiceChanges) => Promise<void>;
  onDirtyChange?: (dirty: boolean) => void;
}) {
  const id = useId();
  const action = useAction();
  const [draft, setDraft] = useState<Draft>({});
  const [saved, setSaved] = useState(false);
  const model = draft.model ?? selection.current.model;
  const effort = draft.reasoning_effort ?? selection.current.reasoning_effort;
  const ask = draft.ask ?? selection.ask_effort ?? "medium";
  const ranking = draft.ranking ?? selection.judge_effort ?? "low";
  const slots = draft.slots ?? jobSlots?.toString() ?? "";
  const slotCount = Number(slots);
  const slotsChanged = draft.slots !== undefined && slotCount !== jobSlots;
  const slotsInvalid =
    slotsChanged && (!Number.isInteger(slotCount) || slotCount < 1 || slotCount > MAX_JOB_SLOTS);
  const changes: LlmChoiceChanges = {};
  if (model !== selection.current.model || effort !== selection.current.reasoning_effort) {
    changes.model = { model, reasoning_effort: effort };
  }
  if (ask !== (selection.ask_effort ?? "medium")) changes.ask = ask;
  if (ranking !== (selection.judge_effort ?? "low")) changes.ranking = ranking;
  if (slotsChanged && !slotsInvalid) changes.slots = slotCount;
  const dirty = slotsChanged || Object.keys(changes).length > 0;
  const canSave = dirty && !slotsInvalid && !action.busy;
  useEffect(() => onDirtyChange?.(dirty), [dirty, onDirtyChange]);
  useEffect(() => () => onDirtyChange?.(false), [onDirtyChange]);
  const save = async () => {
    setSaved(false);
    if (await action.run(() => onSave(changes))) {
      setDraft({});
      setSaved(true);
    }
  };
  const edit = (change: typeof draft) => {
    setDraft({ ...draft, ...change });
    action.setError(null);
    setSaved(false);
  };
  return (
    <Section title="Model and effort">
      <FormRow label="Model" htmlFor={`${id}-model`} help="The model used by new jobs on your linked plan.">
        <select
          id={`${id}-model`}
          value={model}
          disabled={action.busy}
          onChange={(e) =>
            edit({ model: e.target.value === selection.current.model ? undefined : e.target.value })
          }
          className="w-full rounded border border-border bg-surface px-2 py-1"
        >
          {selection.models.map(({ id, label }) => (
            <option key={id} value={id}>
              {label}
            </option>
          ))}
        </select>
      </FormRow>
      {(
        [
          [
            "reasoning_effort",
            "Reasoning effort",
            effort,
            selection.current.reasoning_effort,
            "llm_effort",
            "Reasoning used for new analysis jobs.",
          ],
          [
            "ask",
            "Ask effort",
            ask,
            selection.ask_effort ?? "medium",
            "ask_effort",
            "Reasoning used when answering an Ask question.",
          ],
          [
            "ranking",
            "Dashboard ranking effort",
            ranking,
            selection.judge_effort ?? "low",
            "judge_effort",
            "Reasoning used when ranking My work items.",
          ],
        ] as const
      ).map(([field, label, value, baseline, topic, help]) => (
        <FormRow
          key={field}
          label={label}
          htmlFor={`${id}-${field}`}
          help={
            <>
              {help} <HelpTip topic={topic} />
            </>
          }
        >
          <select
            id={`${id}-${field}`}
            value={value}
            disabled={action.busy}
            onChange={(e) =>
              edit({ [field]: e.target.value === baseline ? undefined : (e.target.value as ReasoningEffort) })
            }
            className="w-full rounded border border-border bg-surface px-2 py-1"
          >
            {selection.efforts.map((option) => (
              <option key={option}>{option}</option>
            ))}
          </select>
        </FormRow>
      ))}
      {jobSlots !== undefined && (
        <FormRow
          label="Concurrent LLM jobs"
          htmlFor={`${id}-slots`}
          help={
            <>
              Your jobs for a pull request run one at a time. <HelpTip topic="llm_job_slots" />
            </>
          }
        >
          <input
            id={`${id}-slots`}
            type="number"
            min={1}
            max={MAX_JOB_SLOTS}
            step={1}
            value={slots}
            disabled={action.busy}
            aria-invalid={slotsInvalid || undefined}
            aria-describedby={slotsInvalid ? `${id}-slots-error` : undefined}
            onChange={(e) =>
              edit({ slots: e.target.value === String(jobSlots) ? undefined : e.target.value })
            }
            className="w-full rounded border border-border bg-surface px-2 py-1"
          />
          {slotsInvalid && (
            <p id={`${id}-slots-error`} className="text-xs">
              Enter a whole number from 1 to 65,535.
            </p>
          )}
        </FormRow>
      )}
      {selection.changed_by && selection.changed_at && (
        <p className="text-xs text-muted">
          changed by {selection.changed_by} at {dateTimeText(selection.changed_at)}
        </p>
      )}
      <div className="grid justify-items-end gap-1">
        <Tooltip tip="Saves your LLM preferences.">
          <button
            type="button"
            aria-disabled={!canSave || undefined}
            className={buttonClass("primary")}
            onClick={() => canSave && void save()}
          >
            {action.busy ? "Saving…" : "Save"}
          </button>
        </Tooltip>
        {saved && <p role="status">Saved</p>}
        {action.error && <p role="alert">Could not save: {action.error}</p>}
      </div>
    </Section>
  );
}
