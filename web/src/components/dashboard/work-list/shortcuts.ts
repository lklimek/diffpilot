// The keyboard map of the dashboard (plan §F.4): the help dialog and the list's key handler both read it.

/** What a dashboard shortcut does. */
export type DashboardShortcut = "next" | "previous" | "toggle" | "review" | "open" | "help" | "close";

/** Shortcuts in help order: `key` is `KeyboardEvent.key`, `label` the key cap, `text` the help line. */
export const DASHBOARD_SHORTCUTS: { key: string; label: string; action: DashboardShortcut; text: string }[] =
  [
    { key: "j", label: "j", action: "next", text: "next item" },
    { key: "k", label: "k", action: "previous", text: "previous item" },
    {
      key: "Enter",
      label: "Enter",
      action: "toggle",
      text: "expand or collapse the score breakdown of the current item",
    },
    {
      key: "v",
      label: "v",
      action: "review",
      text: "review the current PR (opens the review start page or your review)",
    },
    { key: "g", label: "g", action: "open", text: "open the current item on GitHub or in MemCan (new tab)" },
    { key: "?", label: "?", action: "help", text: "this help" },
    { key: "Escape", label: "Esc", action: "close", text: "close a tooltip or this help" },
  ];

/** The action bound to `KeyboardEvent.key`, if any. */
export const dashboardShortcut = (key: string): DashboardShortcut | undefined =>
  DASHBOARD_SHORTCUTS.find((s) => s.key === key)?.action;

/** Whether a key press belongs to a text field or carries a modifier (shortcuts stay off then). */
export function isTyping(event: KeyboardEvent): boolean {
  if (event.ctrlKey || event.metaKey || event.altKey) return true;
  const target = event.target;
  if (!(target instanceof HTMLElement)) return false;
  return target.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName);
}
