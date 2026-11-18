import { useId } from "react";
import { ERROR, FIELD } from "./look";
import type { FieldErrors } from "./validation";

/** Times of day in 15-minute steps (plan §F.6 G15). */
const TIMES = Array.from({ length: 96 }, (_, i) => {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(Math.floor(i / 4))}:${pad((i % 4) * 15)}`;
});

/** IANA zones the browser knows; a stored zone outside the list is still offered. */
const ZONES: string[] = (() => {
  try {
    return Intl.supportedValuesOf("timeZone");
  } catch {
    return [];
  }
})();

interface ScheduleGroupProps {
  enabled: boolean;
  /** `HH:MM`. */
  time: string;
  /** IANA name; empty when none is stored yet. */
  timeZone: string;
  errors: FieldErrors;
  onChange: (change: { schedule_enabled?: boolean; schedule_time?: string; time_zone?: string }) => void;
}

/** Daily automatic refresh (plan §F.6 G14–G16): on/off, local time, time zone. */
export function ScheduleGroup({ enabled, time, timeZone, errors, onChange }: ScheduleGroupProps) {
  const id = useId();
  const times = TIMES.includes(time) ? TIMES : [time, ...TIMES];
  const zones = timeZone === "" || ZONES.includes(timeZone) ? ZONES : [timeZone, ...ZONES];
  return (
    <div className="grid content-start gap-3">
      <label className="flex items-center gap-2 text-sm">
        <input
          type="checkbox"
          checked={enabled}
          className="size-4"
          onChange={(e) => onChange({ schedule_enabled: e.target.checked })}
        />
        Refresh my dashboard automatically every day
      </label>
      <div className="flex flex-wrap gap-4">
        <div className="grid gap-1">
          <label htmlFor={`${id}-time`} className="text-sm font-semibold">
            Time of day
          </label>
          <select
            id={`${id}-time`}
            value={time}
            aria-invalid={errors.schedule_time ? true : undefined}
            aria-describedby={errors.schedule_time ? `${id}-time-error` : undefined}
            className={`${FIELD} w-28`}
            onChange={(e) => onChange({ schedule_time: e.target.value })}
          >
            {times.map((t) => (
              <option key={t} value={t}>
                {t}
              </option>
            ))}
          </select>
        </div>
        <div className="grid gap-1">
          <label htmlFor={`${id}-zone`} className="text-sm font-semibold">
            Time zone
          </label>
          <select
            id={`${id}-zone`}
            value={timeZone}
            aria-describedby={timeZone === "" ? `${id}-zone-none` : undefined}
            className={`${FIELD} w-64`}
            onChange={(e) => onChange({ time_zone: e.target.value })}
          >
            {timeZone === "" && <option value="">Not set yet</option>}
            {zones.map((zone) => (
              <option key={zone} value={zone}>
                {zone}
              </option>
            ))}
          </select>
        </div>
      </div>
      {errors.schedule_time && (
        <p id={`${id}-time-error`} className={ERROR}>
          {errors.schedule_time}
        </p>
      )}
      {timeZone === "" && (
        <p id={`${id}-zone-none`} className="text-xs text-muted">
          No time zone is stored yet, so the automatic refresh does not run. Choose one here, or open My work
          once in this browser.
        </p>
      )}
      <p className="text-xs text-muted">
        Runs once a day at this local time with your GitHub sign-in. A manual refresh after that time counts
        for the day.
      </p>
    </div>
  );
}
