// Button look of docs/mockups/mockup.css `.btn` (`.pri`, `.danger`, `.sm`) for buttons and button-like links.

/** Button variant: primary action, destructive action or plain. */
export type ButtonVariant = "primary" | "danger" | "plain";

const LOOK: Record<ButtonVariant, string> = {
  primary: "border-accent bg-accent font-semibold text-on-accent",
  danger: "border-high-bd bg-surface text-high hover:bg-high-bg",
  plain: "border-border bg-surface text-text hover:border-accent",
};

/** Classes of a button or button-like link; `small` is the banner size. */
export function buttonClass(variant: ButtonVariant, small = false): string {
  const size = small ? "px-2 py-0.5 text-xs" : "px-3 py-1.5";
  return `inline-flex cursor-pointer items-center gap-1.5 rounded-md border whitespace-nowrap no-underline disabled:cursor-not-allowed disabled:opacity-55 ${size} ${LOOK[variant]}`;
}
