import { useId } from "react";
import { HelpTip } from "../../settings/HelpTip";
import { ListEditor } from "./ListEditor";
import { ERROR, FIELD } from "./look";
import type { FieldErrors } from "./validation";

interface RulesGroupProps {
  lookbackDays: string;
  readyLabel: string;
  skipAuthors: string[];
  knownBots: string[];
  errors: FieldErrors;
  /** Why a bot login cannot be added, or null. */
  checkAuthor: (value: string, items: string[]) => string | null;
  /** Why a known bot cannot be added, or null. */
  checkBot: (value: string, items: string[]) => string | null;
  onChange: (change: {
    lookback_days?: string;
    ready_label?: string;
    skip_authors?: string[];
    known_bots?: string[];
  }) => void;
}

/** Ranking rules of plan §F.6 G9–G11: look-back days, the ready label, excluded authors and known bots. */
export function RulesGroup({
  lookbackDays,
  readyLabel,
  skipAuthors,
  knownBots,
  errors,
  checkAuthor,
  checkBot,
  onChange,
}: RulesGroupProps) {
  const id = useId();
  return (
    <div className="grid content-start gap-3">
      <div className="grid gap-1">
        <span className="flex items-center gap-2">
          <label htmlFor={`${id}-days`} className="text-sm font-semibold">
            Look-back days
          </label>
          <HelpTip topic="lookback_days" />
        </span>
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
        <span className="flex items-center gap-2">
          <label htmlFor={`${id}-label`} className="text-sm font-semibold">
            &quot;Ready for a human&quot; label
          </label>
          <HelpTip topic="ready_label" />
        </span>
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
        label="Excluded authors (never shown)"
        addLabel="Add an excluded author"
        help={<HelpTip topic="skip_authors" />}
        hint="Pull requests and issues written by these accounts are never shown in Needs you, also when a review is requested from you or they are assigned to you."
        items={skipAuthors}
        empty="None: items of every author are ranked."
        placeholder="renovate[bot]"
        check={checkAuthor}
        error={errors.skip_authors}
        onChange={(skip_authors) => onChange({ skip_authors })}
      />
      <ListEditor
        label="Known bots"
        addLabel="Add a known bot"
        help={<HelpTip topic="known_bots" />}
        hint="Treated as bots: their review requests and comments do not count as a person waiting for you. GitHub Apps (…[bot]) are always bots."
        items={knownBots}
        empty="None: only GitHub Apps count as bots."
        placeholder="review-bot"
        check={checkBot}
        error={errors.known_bots}
        onChange={(known_bots) => onChange({ known_bots })}
      />
    </div>
  );
}
