import { useEffect, useId, useRef } from "react";
import { buttonClass } from "../../button/button";
import { Tooltip } from "../../tooltip/Tooltip";

interface UnsavedChangesPromptProps {
  onStay: () => void;
  onLeave: () => void;
}

/** Asked when leaving Settings with unsaved settings; Stay takes the focus. */
export function UnsavedChangesPrompt({ onStay, onLeave }: UnsavedChangesPromptProps) {
  const id = useId();
  const stay = useRef<HTMLButtonElement>(null);
  useEffect(() => stay.current?.focus(), []);
  return (
    <div
      role="alertdialog"
      aria-label="Unsaved settings"
      aria-describedby={id}
      className="grid gap-2 rounded-md border border-l-4 border-med-bd bg-med-bg px-3.5 py-2.5 text-text"
    >
      <p id={id}>This page has unsaved changes. Leave without saving them?</p>
      <div className="flex gap-2">
        <Tooltip tip="Stays on Settings with your changes.">
          <button ref={stay} type="button" className={buttonClass("primary")} onClick={onStay}>
            Stay
          </button>
        </Tooltip>
        <Tooltip tip="Discards the unsaved changes on this page.">
          <button type="button" className={buttonClass("danger")} onClick={onLeave}>
            Leave without saving
          </button>
        </Tooltip>
      </div>
    </div>
  );
}
