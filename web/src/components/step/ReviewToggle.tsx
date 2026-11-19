import type { ReactNode } from "react";
import { Tooltip } from "../tooltip/Tooltip";

interface ReviewToggleProps {
  /** Button text; stays the same in both states (the pressed state carries the tick). */
  label: ReactNode;
  /** What the button does. */
  tip: string;
  /** Whether it is ticked. */
  pressed: boolean;
  /** Called with the new state. */
  onChange: (pressed: boolean) => void;
}

/** Tick button ("Mark reviewed"): a toggle with `aria-pressed`. */
export function ReviewToggle({ label, tip, pressed, onChange }: ReviewToggleProps) {
  return (
    <Tooltip tip={tip}>
      <button
        type="button"
        aria-pressed={pressed}
        onClick={() => onChange(!pressed)}
        className="cursor-pointer rounded-md border border-border bg-surface px-2.5 py-1 text-[12.5px] font-semibold whitespace-nowrap text-text hover:border-accent aria-pressed:border-accent aria-pressed:bg-accent-bg aria-pressed:text-accent"
      >
        <span aria-hidden className="mr-1">
          {pressed ? "☑" : "☐"}
        </span>
        {label}
      </button>
    </Tooltip>
  );
}
