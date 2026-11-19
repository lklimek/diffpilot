import type { ReactNode } from "react";
import { Tooltip } from "../../tooltip/Tooltip";

/** One toggle of a toggle group. */
export interface ToggleOption<T extends string> {
  value: T;
  label: ReactNode;
  tip: string;
}

interface ToggleGroupProps<T extends string> {
  /** Accessible name of the group. */
  label: string;
  options: ToggleOption<T>[];
  /** The toggles on, in any order. */
  values: T[];
  /** The toggles on, in option order. */
  onChange: (values: T[]) => void;
  /** Why the last toggle on cannot be turned off. */
  lastOnTip?: string;
}

const LAST_ON_TIP = "At least one kind stays shown; turn another on first.";

/** Independent toggle buttons (`aria-pressed`); the last one on cannot be turned off. */
export function ToggleGroup<T extends string>({
  label,
  options,
  values,
  onChange,
  lastOnTip = LAST_ON_TIP,
}: ToggleGroupProps<T>) {
  const on = options.filter((option) => values.includes(option.value));
  return (
    <div
      role="group"
      aria-label={label}
      className="inline-flex overflow-hidden rounded-md border border-border [&>*+*>button]:border-l [&>*+*>button]:border-border"
    >
      {options.map((option) => {
        const pressed = values.includes(option.value);
        const last = pressed && on.length === 1;
        return (
          <Tooltip
            key={option.value}
            tip={last ? `${option.tip}. ${lastOnTip}` : option.tip}
            className="flex"
          >
            <button
              type="button"
              aria-pressed={pressed}
              aria-disabled={last || undefined}
              onClick={() => {
                if (last) return;
                const next = pressed ? values.filter((v) => v !== option.value) : [...values, option.value];
                onChange(options.map((o) => o.value).filter((v) => next.includes(v)));
              }}
              className="min-h-[26px] cursor-pointer bg-surface px-2.5 py-0.5 text-[12.5px] text-text aria-disabled:cursor-default aria-pressed:bg-accent aria-pressed:font-semibold aria-pressed:text-on-accent"
            >
              {option.label}
            </button>
          </Tooltip>
        );
      })}
    </div>
  );
}
