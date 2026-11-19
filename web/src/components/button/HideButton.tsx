import { Tooltip } from "../tooltip/Tooltip";
import { HEAD_BUTTON } from "./button";

interface HideButtonProps {
  /** Accessible name: "Hide <panel>". */
  name: string;
  /** What hiding does and how to show the panel again. */
  tip: string;
  /** Hides the panel. */
  onHide: () => void;
  /** The control that shows the panel again: it gets the focus, as this button leaves with its panel. */
  focusAfter?: () => HTMLElement | null | undefined;
}

/** "Hide" in the top-right corner of an open panel: the same button as a column head's Hide. */
export function HideButton({ name, tip, onHide, focusAfter }: HideButtonProps) {
  return (
    <Tooltip tip={tip}>
      <button
        type="button"
        aria-label={name}
        className={HEAD_BUTTON}
        onClick={() => {
          focusAfter?.()?.focus();
          onHide();
        }}
      >
        Hide
      </button>
    </Tooltip>
  );
}
