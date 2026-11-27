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
}

/** The installation-wide model and efforts, saved together. */
export function LlmSelectionCard({
  selection,
  onSave,
  onDirtyChange,
}: {
  selection: LlmSelection;
  onSave: (changes: LlmChoiceChanges) => Promise<void>;
  onDirtyChange?: (dirty: boolean) => void;
}) {
  const id = useId();
  const action = useAction();
  const [draft, setDraft] = useState<Partial<ModelChoice> & Omit<LlmChoiceChanges, "model">>({});
  const [saved, setSaved] = useState(false);
  const model = draft.model ?? selection.current.model;
  const effort = draft.reasoning_effort ?? selection.current.reasoning_effort;
  const ask = draft.ask ?? selection.ask_effort ?? "medium";
  const ranking = draft.ranking ?? selection.judge_effort ?? "low";
  const changes: LlmChoiceChanges = {};
  if (model !== selection.current.model || effort !== selection.current.reasoning_effort) {
    changes.model = { model, reasoning_effort: effort };
  }
  if (ask !== (selection.ask_effort ?? "medium")) changes.ask = ask;
  if (ranking !== (selection.judge_effort ?? "low")) changes.ranking = ranking;
  const dirty = Object.keys(changes).length > 0;
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
      <FormRow
        label="Model"
        htmlFor={`${id}-model`}
        help="The model used by new jobs across this installation."
      >
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
      {selection.changed_by && selection.changed_at && (
        <p className="text-xs text-muted">
          changed by {selection.changed_by} at {dateTimeText(selection.changed_at)}
        </p>
      )}
      <div className="grid justify-items-end gap-1">
        <Tooltip tip="Saves the changed model and efforts for this installation.">
          <button
            type="button"
            aria-disabled={!dirty || action.busy || undefined}
            className={buttonClass("primary")}
            onClick={() => dirty && void save()}
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
