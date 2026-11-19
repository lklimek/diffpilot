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
  | "close"
  | "switch_view"
  | "toggle_viewed";

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
  { key: "s", label: "s", action: "switch_view", text: "switch to the Files view at this step's lines" },
  { key: "?", label: "?", action: "help", text: "this help" },
  { key: "Escape", label: "Esc", action: "close", text: "close" },
];

/** The keyboard map of the Files view (brief §4.5): hunks and files instead of blocks; no ticking. */
export const FILES_SHORTCUTS: typeof SHORTCUTS = [
  { key: "j", label: "j", action: "next_block", text: "next hunk (continues into the next file)" },
  { key: "k", label: "k", action: "previous_block", text: "previous hunk" },
  { key: "n", label: "n", action: "next_flow", text: "next file" },
  { key: "p", label: "p", action: "previous_flow", text: "previous file" },
  {
    key: "v",
    label: "v",
    action: "toggle_viewed",
    text: "mark the file Viewed on GitHub (then the next file opens), or unmark it",
  },
  { key: "r", label: "r", action: "next_remark", text: "next hunk with LLM remarks" },
  { key: "d", label: "d", action: "toggle_old_code", text: "show / hide old code of the file" },
  {
    key: "c",
    label: "c",
    action: "comment",
    text: "comment on the selection's last line or the hunk's first line",
  },
  { key: "a", label: "a", action: "ask", text: "ask about the selected lines, hunk or file" },
  { key: "s", label: "s", action: "switch_view", text: "switch to the Flows view at this place" },
  { key: "?", label: "?", action: "help", text: "this help" },
  {
    key: "Escape",
    label: "Esc",
    action: "close",
    text: "close: step details (back to the hunk), then the selection (back to the file)",
  },
];

/** The action bound to `KeyboardEvent.key` in a key map (the Flows view's by default), if any. */
export const shortcutAction = (key: string, map = SHORTCUTS): ShortcutAction | undefined =>
  map.find((s) => s.key === key)?.action;
