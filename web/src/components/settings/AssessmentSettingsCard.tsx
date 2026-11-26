import { AssessmentQuotaText } from "../comments/AssessmentQuotaText";
import { Tooltip } from "../tooltip/Tooltip";
import { useEffect, useId, useState } from "react";
import type { AssessmentConfig, AssessmentMethod, AssessmentSettings } from "../../api/generated/types";
import { buttonClass } from "../button/button";
import { FormRow } from "./FormRow";
import { SettingsCard } from "./Card";

/** User choices apply to new passes; saving never schedules an assessment. */
export function AssessmentSettingsCard({
  settings,
  now,
  busy,
  error,
  saved,
  onSave,
  onDirtyChange,
}: {
  settings: AssessmentSettings;
  now: Date;
  busy: boolean;
  error?: string;
  saved?: boolean;
  onSave: (config: AssessmentConfig) => void;
  onDirtyChange?: (dirty: boolean) => void;
}) {
  const id = useId();
  const [original] = useState(settings.config);
  const [primary, setPrimary] = useState(settings.config.primary_method);
  const [deadline, setDeadline] = useState(settings.config.ab_until ?? "");
  const parsed = Date.parse(deadline);
  const valid =
    !deadline ||
    (/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z$/.test(deadline) &&
      Number.isFinite(parsed) &&
      new Date(parsed).toISOString().slice(0, 19) === deadline.slice(0, 19));
  const dirty = primary !== settings.config.primary_method || deadline !== (settings.config.ab_until ?? "");
  useEffect(() => onDirtyChange?.(dirty), [dirty, onDirtyChange]);
  useEffect(() => () => onDirtyChange?.(false), [onDirtyChange]);
  return (
    <SettingsCard title="Comment assessment">
      <FormRow
        htmlFor={`${id}-primary`}
        label="Primary assessment method"
        help="Comment cards show only the primary assessment. Changes apply to new passes and do not start an assessment."
      >
        <select
          id={`${id}-primary`}
          disabled={busy}
          value={primary}
          onChange={(e) => setPrimary(e.target.value as AssessmentMethod)}
          className="rounded border border-border bg-surface px-2 py-1"
        >
          <option value="claudius_host">claudius_host (default)</option>
          <option value="prompt_v4">prompt_v4</option>
        </select>
      </FormRow>
      <p>A/B runs both methods and costs roughly 2× in tokens and time. A pair uses one daily pass.</p>
      <p>
        <AssessmentQuotaText quota={settings.quota} />
      </p>
      <p>Toggle A/B for each PR on its Comments page. Selected PRs:</p>
      {settings.config.ab_prs.length ? (
        <ul className="list-disc pl-5">
          {settings.config.ab_prs.map((pr) => (
            <li key={pr}>{pr}</li>
          ))}
        </ul>
      ) : (
        <p>No PRs selected.</p>
      )}
      <FormRow
        htmlFor={`${id}-deadline`}
        label="Optional A/B deadline (UTC)"
        help="Leave empty for selected PRs only. A deadline enables both methods on all PRs until that time, in addition to selected PRs."
      >
        <input
          id={`${id}-deadline`}
          type="text"
          placeholder="2026-11-30T00:00:00Z"
          value={deadline}
          disabled={busy}
          aria-invalid={!valid || undefined}
          onChange={(e) => setDeadline(e.target.value)}
          className="rounded border border-border bg-surface px-2 py-1"
        />
      </FormRow>
      {valid && deadline && parsed <= now.getTime() && (
        <p>The A/B deadline {deadline} has passed. Only selected PRs use both methods.</p>
      )}
      {!valid && <p role="alert">Enter a UTC date and time ending in Z, or leave empty.</p>}
      {original.primary_method !== settings.config.primary_method && (
        <p>
          Primary method changed elsewhere: {original.primary_method} → {settings.config.primary_method}.
        </p>
      )}
      {original.ab_until !== settings.config.ab_until && (
        <p>
          A/B deadline changed elsewhere: {original.ab_until ?? "none"} → {settings.config.ab_until ?? "none"}
          .
        </p>
      )}
      {original.ab_prs.join(", ") !== settings.config.ab_prs.join(", ") && (
        <p>
          Selected PRs changed elsewhere: {original.ab_prs.join(", ") || "none"} →{" "}
          {settings.config.ab_prs.join(", ") || "none"}.
        </p>
      )}
      {error && <p role="alert">{error}</p>}
      {saved && <p role="status">Assessment settings saved.</p>}
      <Tooltip
        tip={
          busy
            ? "Saving assessment settings. Wait for it to finish."
            : !valid
              ? "Enter a UTC date and time ending in Z, or leave empty."
              : "Saves choices for new passes without starting an assessment."
        }
      >
        <button
          type="button"
          aria-disabled={busy || !valid || undefined}
          className={`${buttonClass("primary", true)} justify-self-end`}
          onClick={() =>
            !busy &&
            valid &&
            onSave({
              ...settings.config,
              primary_method: primary,
              ab_until: deadline ? new Date(deadline).toISOString() : null,
            })
          }
        >
          Save assessment settings
        </button>
      </Tooltip>
    </SettingsCard>
  );
}
