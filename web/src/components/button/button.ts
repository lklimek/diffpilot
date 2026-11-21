// Button look of docs/mockups/mockup.css `.btn` (`.pri`, `.danger`, `.sm`) for buttons and button-like links.

/** Button variant: primary action, destructive action or plain. */
export type ButtonVariant = "primary" | "danger" | "plain";

const LOOK: Record<ButtonVariant, string> = {
  primary: "border-accent bg-accent font-semibold text-on-accent",
  danger: "border-high-bd bg-surface text-high hover:bg-high-bg",
  plain: "border-border bg-surface text-text hover:border-accent",
};

/**
 * Tip suffix of a control that is unavailable while a request runs. Controls with a tooltip are never natively
 * disabled (the tip would be unreachable by keyboard): they use `aria-disabled`, stay focusable, ignore
 * activation, and their tip says why.
 */
export const BUSY_NOTE = "Not available while the current request runs.";

/** The tip of a control, with the reason when it is unavailable because a request runs. */
export const busyTip = (tip: string, busy: boolean) => (busy ? `${tip} ${BUSY_NOTE}` : tip);

/** Classes of a button or button-like link; `small` is the banner size. */
export function buttonClass(variant: ButtonVariant, small = false): string {
  const size = small ? "px-2 py-0.5 text-xs" : "px-3 py-1.5";
  return `inline-flex cursor-pointer items-center gap-1.5 rounded-md border whitespace-nowrap no-underline disabled:cursor-not-allowed disabled:opacity-55 aria-disabled:cursor-not-allowed aria-disabled:opacity-55 ${size} ${LOOK[variant]}`;
}

/** Look of the column-head buttons (Hide, Full screen); every panel's Hide uses it too. */
export const HEAD_BUTTON =
  "cursor-pointer rounded border border-border bg-surface px-1.5 text-[11.5px] text-text hover:border-accent aria-pressed:border-accent aria-pressed:bg-accent-bg";
