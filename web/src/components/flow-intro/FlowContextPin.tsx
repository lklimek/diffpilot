import type { Ref } from "react";
import { Tooltip } from "../tooltip/Tooltip";

export interface FlowContextPinProps {
  pinned: boolean;
  onToggle: () => void;
  buttonRef: Ref<HTMLButtonElement>;
}

/** Keeps a flow context section visible above the review's scrolling code. */
export function FlowContextPin({
  section,
  pinned,
  onToggle,
  buttonRef,
}: FlowContextPinProps & { section: string }) {
  return (
    <Tooltip
      tip={pinned ? "Returns this section to the scrolling review." : "Keeps this section above the code."}
    >
      <button
        ref={buttonRef}
        type="button"
        aria-label={`Pin ${section}`}
        aria-pressed={pinned}
        onClick={onToggle}
        className="min-h-6 shrink-0 cursor-pointer rounded border border-border px-1.5 text-[11.5px] hover:border-accent aria-pressed:border-accent aria-pressed:text-accent"
      >
        {pinned ? "Pinned" : "Pin"}
      </button>
    </Tooltip>
  );
}
