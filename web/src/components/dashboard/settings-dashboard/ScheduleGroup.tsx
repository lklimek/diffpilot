import { useId } from "react";
import { FormRow } from "../../settings/FormRow";
import { HelpTip } from "../../settings/HelpTip";
import { ERROR, FIELD } from "./look";
import type { FieldErrors } from "./validation";

/** Times of day in 15-minute steps (plan §F.6 G15). */
const TIMES = Array.from({ length: 96 }, (_, i) => {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(Math.floor(i / 4))}:${pad((i % 4) * 15)}`;
});

const ZONES: string[] = (() => {
  try {
    return Intl.supportedValuesOf("timeZone");
  } catch {
    return [];
  }
})();

interface ScheduleGroupProps {
  enabled: boolean;
  time: string;
  timeZone: string;
  errors: FieldErrors;
  onChange: (change: { schedule_enabled?: boolean; schedule_time?: string; time_zone?: string }) => void;
}

/** Nightly scan switch, local time and time zone. */
export function ScheduleGroup({ enabled, time, timeZone, errors, onChange }: ScheduleGroupProps) {
  const id = useId();
  const times = TIMES.includes(time) ? TIMES : [time, ...TIMES];
  const zones = timeZone === "" || ZONES.includes(timeZone) ? ZONES : [timeZone, ...ZONES];
  return (
    <>
      <p className="text-xs text-muted">
        Runs once a night; judges only items never judged. Updates in between come from GitHub events and
        Refresh.
      </p>
      <FormRow label="Run nightly scan" htmlFor={`${id}-enabled`} help={<HelpTip topic="schedule_enabled" />}>
        <input
          id={`${id}-enabled`}
          type="checkbox"
          checked={enabled}
          className="size-6"
          onChange={(e) => onChange({ schedule_enabled: e.target.checked })}
        />
      </FormRow>
      <FormRow label="Time of day" htmlFor={`${id}-time`} help={<HelpTip topic="schedule_time" />}>
        <select
          id={`${id}-time`}
          value={time}
          aria-invalid={!!errors.schedule_time || undefined}
          aria-describedby={errors.schedule_time ? `${id}-time-error` : undefined}
          className={`${FIELD} w-28 max-w-full`}
          onChange={(e) => onChange({ schedule_time: e.target.value })}
        >
          {times.map((t) => (
            <option key={t}>{t}</option>
          ))}
        </select>
        {errors.schedule_time && (
          <p id={`${id}-time-error`} className={ERROR}>
            {errors.schedule_time}
          </p>
        )}
      </FormRow>
      <FormRow
        label="Time zone"
        htmlFor={`${id}-zone`}
        help={
          timeZone === "" ? (
            <span id={`${id}-zone-none`}>
              The nightly scan needs a time zone. Choose one here, or open My work once in this browser.
            </span>
          ) : (
            <HelpTip topic="time_zone" />
          )
        }
      >
        <select
          id={`${id}-zone`}
          value={timeZone}
          aria-describedby={timeZone === "" ? `${id}-zone-none` : undefined}
          className={`${FIELD} w-64 max-w-full`}
          onChange={(e) => onChange({ time_zone: e.target.value })}
        >
          {timeZone === "" && <option value="">Not set yet</option>}
          {zones.map((zone) => (
            <option key={zone}>{zone}</option>
          ))}
        </select>
      </FormRow>
    </>
  );
}
