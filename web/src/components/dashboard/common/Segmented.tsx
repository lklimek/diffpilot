import type { ReactNode } from "react";
import { Tooltip } from "../../tooltip/Tooltip";

/** One option of a segmented switch. */
export interface SegmentOption<T extends string> {
  value: T;
  label: ReactNode;
  tip: string;
}

interface SegmentedProps<T extends string> {
  /** Accessible name of the group. */
  label: string;
  options: SegmentOption<T>[];
  value: T;
  onChange: (value: T) => void;
}

/** A row of toggle buttons (`aria-pressed`), one of them on. */
export function Segmented<T extends string>({ label, options, value, onChange }: SegmentedProps<T>) {
  return (
    <div
      role="group"
      aria-label={label}
      className="inline-flex overflow-hidden rounded-md border border-border [&>*+*>button]:border-l [&>*+*>button]:border-border"
    >
      {options.map((option) => (
        <Tooltip key={option.value} tip={option.tip} className="flex">
          <button
            type="button"
            aria-pressed={option.value === value}
            onClick={() => onChange(option.value)}
            className="min-h-[26px] cursor-pointer bg-surface px-2.5 py-0.5 text-[12.5px] text-text aria-pressed:bg-accent aria-pressed:font-semibold aria-pressed:text-on-accent"
          >
            {option.label}
          </button>
        </Tooltip>
      ))}
    </div>
  );
}
