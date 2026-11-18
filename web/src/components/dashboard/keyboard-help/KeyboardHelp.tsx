import { useEffect, useId, useRef, type KeyboardEvent } from "react";
import { buttonClass } from "../../button/button";
import { Tooltip } from "../../tooltip/Tooltip";
import { DASHBOARD_SHORTCUTS } from "../work-list/shortcuts";

interface KeyboardHelpProps {
  open: boolean;
  /** Close, Escape or a click outside. */
  onClose: () => void;
}

/** The dashboard's `?` help: its keyboard shortcuts (modal; focus returns to where it was). */
export function KeyboardHelp({ open, onClose }: KeyboardHelpProps) {
  const titleId = useId();
  const dialog = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const previous = document.activeElement as HTMLElement | null;
    dialog.current?.focus();
    return () => previous?.focus();
  }, [open]);

  if (!open) return null;

  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key === "Escape") {
      event.preventDefault();
      onClose();
    } else if (event.key === "Tab") {
      // One control: keep the focus inside the dialog.
      event.preventDefault();
      dialog.current?.querySelector<HTMLElement>("button")?.focus();
    }
  };

  return (
    <div className="fixed inset-0 z-50 grid place-items-center p-4">
      <div
        aria-hidden
        data-testid="keyboard-help-backdrop"
        className="absolute inset-0 bg-black/40"
        onClick={onClose}
      />
      <div
        ref={dialog}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
        onKeyDown={onKeyDown}
        className="relative w-full max-w-lg rounded-lg border border-border bg-surface px-5 py-4 text-sm text-text shadow-tip"
      >
        <h2 id={titleId} className="mb-2 text-lg font-bold">
          Keyboard shortcuts
        </h2>
        <table aria-labelledby={titleId} className="w-full border-collapse">
          <tbody>
            {DASHBOARD_SHORTCUTS.map(({ label, text }) => (
              <tr key={label} className="border-b border-border last:border-0">
                <td className="py-1 pr-3 whitespace-nowrap">
                  <kbd className="rounded border border-b-2 border-border px-1 font-mono text-xs">
                    {label}
                  </kbd>
                </td>
                <td className="py-1">{text}</td>
              </tr>
            ))}
          </tbody>
        </table>
        <p className="mt-2.5 text-xs text-muted">
          None while you type in a field. Refresh has no shortcut on purpose: a scan takes 1–2 minutes.
        </p>
        <Tooltip tip="Close this help (Esc)">
          <button type="button" onClick={onClose} className={`${buttonClass("primary")} mt-3`}>
            Close
          </button>
        </Tooltip>
      </div>
    </div>
  );
}
