import { useId } from "react";
import { ListEditor } from "./ListEditor";
import { ERROR, FIELD } from "./look";
import type { FieldErrors } from "./validation";

interface RulesGroupProps {
  lookbackDays: string;
  readyLabel: string;
  skipAuthors: string[];
  errors: FieldErrors;
  /** Why a bot login cannot be added, or null. */
  checkAuthor: (value: string, items: string[]) => string | null;
  onChange: (change: { lookback_days?: string; ready_label?: string; skip_authors?: string[] }) => void;
}

/** Ranking rules of plan §F.6 G9–G11: look-back days, the ready label, bot authors to skip. */
export function RulesGroup({
  lookbackDays,
  readyLabel,
  skipAuthors,
  errors,
  checkAuthor,
  onChange,
}: RulesGroupProps) {
  const id = useId();
  return (
    <div className="grid content-start gap-3">
      <div className="grid gap-1">
        <label htmlFor={`${id}-days`} className="text-sm font-semibold">
          Look-back days
        </label>
        <input
          id={`${id}-days`}
          type="number"
          min={1}
          max={30}
          step={1}
          value={lookbackDays}
          aria-invalid={errors.lookback_days ? true : undefined}
          aria-describedby={`${id}-days-help${errors.lookback_days ? ` ${id}-days-error` : ""}`}
          className={`${FIELD} w-24`}
          onChange={(e) => onChange({ lookback_days: e.target.value })}
        />
        <p id={`${id}-days-help`} className="text-xs text-muted">
          An issue opened within this many days is marked new (1 to 30).
        </p>
        {errors.lookback_days && (
          <p id={`${id}-days-error`} className={ERROR}>
            {errors.lookback_days}
          </p>
        )}
      </div>
      <div className="grid gap-1">
        <label htmlFor={`${id}-label`} className="text-sm font-semibold">
          &quot;Ready for a human&quot; label
        </label>
        <input
          id={`${id}-label`}
          value={readyLabel}
          autoComplete="off"
          aria-invalid={errors.ready_label ? true : undefined}
          aria-describedby={`${id}-label-help${errors.ready_label ? ` ${id}-label-error` : ""}`}
          className={`${FIELD} w-56 font-mono`}
          onChange={(e) => onChange({ ready_label: e.target.value })}
        />
        <p id={`${id}-label-help`} className="text-xs text-muted">
          A pull request with this label counts as ready for review.
        </p>
        {errors.ready_label && (
          <p id={`${id}-label-error`} className={ERROR}>
            {errors.ready_label}
          </p>
        )}
      </div>
      <ListEditor
        label="Bot authors to skip"
        addLabel="Add a bot author to skip"
        items={skipAuthors}
        empty="None: items of every author are ranked."
        placeholder="renovate[bot]"
        check={checkAuthor}
        error={errors.skip_authors}
        onChange={(skip_authors) => onChange({ skip_authors })}
      />
    </div>
  );
}
