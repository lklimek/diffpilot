// The keyboard map of the review screen (brief §4.3): the help rows and the screen's key handler both read it.

/** What a shortcut does. */
export type ShortcutAction =
  | "next_block"
  | "previous_block"
  | "next_flow"
  | "previous_flow"
  | "tick_step"
  | "toggle_old_code"
  | "comment"
  | "ask"
  | "next_remark"
  | "help"
  | "close";

/** Shortcuts in help order: `key` is `KeyboardEvent.key`, `label` the key cap, `text` the help line. */
export const SHORTCUTS: { key: string; label: string; action: ShortcutAction; text: string }[] = [
  {
    key: "j",
    label: "j",
    action: "next_block",
    text: "next block (including Not analysed and pending review); on a step it also ticks it as reviewed",
  },
  { key: "k", label: "k", action: "previous_block", text: "previous block" },
  { key: "n", label: "n", action: "next_flow", text: "next flow; after the last flow, next quiet block" },
  { key: "p", label: "p", action: "previous_flow", text: "previous flow" },
  { key: "x", label: "x", action: "tick_step", text: "tick the current step as reviewed" },
  { key: "d", label: "d", action: "toggle_old_code", text: "show / hide old code of the current step" },
  { key: "c", label: "c", action: "comment", text: "comment on the current line" },
  { key: "a", label: "a", action: "ask", text: "ask about the current step" },
  {
    key: "r",
    label: "r",
    action: "next_remark",
    text: "next step with LLM remarks (its first remark flashes)",
  },
  { key: "?", label: "?", action: "help", text: "this help" },
  { key: "Escape", label: "Esc", action: "close", text: "close" },
];

/** The action bound to `KeyboardEvent.key`, if any. */
export const shortcutAction = (key: string): ShortcutAction | undefined =>
  SHORTCUTS.find((s) => s.key === key)?.action;
