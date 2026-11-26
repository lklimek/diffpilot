import { AssessmentQuotaText } from "./AssessmentQuotaText";
import type { AssessmentConfig, AssessmentSettings } from "../../api/generated/types";

/** Explicit selection affects future passes only. */
export function AssessmentControls({
  settings,
  now,
  prKey,
  busy,
  error,
  onSave,
}: {
  settings: AssessmentSettings;
  now: Date;
  prKey: string;
  busy: boolean;
  error?: string;
  onSave: (config: AssessmentConfig) => void;
}) {
  return (
    <div className="my-3 grid gap-1 text-sm">
      <label className="flex items-center gap-2">
        <input
          type="checkbox"
          checked={settings.config.ab_prs.some((pr) => pr.toLowerCase() === prKey.toLowerCase())}
          disabled={busy}
          onChange={(e) =>
            onSave({
              ...settings.config,
              ab_prs: e.target.checked
                ? [...settings.config.ab_prs, prKey]
                : settings.config.ab_prs.filter((pr) => pr.toLowerCase() !== prKey.toLowerCase()),
            })
          }
        />
        Compare both methods on this PR
      </label>
      <p>
        Primary for new passes: {settings.config.primary_method}. A/B costs roughly 2× in tokens and time; a
        pair uses one daily pass. Changing this does not start an assessment.
      </p>
      <p>
        <AssessmentQuotaText quota={settings.quota} />
      </p>
      {settings.config.ab_until &&
        (Date.parse(settings.config.ab_until) <= now.getTime() ? (
          <p>The A/B deadline {settings.config.ab_until} has passed.</p>
        ) : (
          <p>
            The configured deadline also enables A/B on all PRs until {settings.config.ab_until}, even if this
            PR is not selected.
          </p>
        ))}
      {error && <p role="alert">{error}</p>}
    </div>
  );
}
