import type { ChatGeometry, ChatSize } from "../../components/ask/chat/AskChat";

/** Narrower windows get the chat as a full-width bottom sheet (proposal AC14). */
export const NARROW_PX = 1100;
const GAP = 8;
const MIN_WIDTH = 320;
const MIN_HEIGHT = 240;
const HIDDEN_INSPECTOR_WIDTH = 460;

const clamp = (value: number, min: number, max: number) => Math.min(max, Math.max(min, value));

/**
 * Where the chat is drawn (proposal AC2): over the Inspector, as wide as it, its bottom on top of the Inspector's
 * dock, 75 % of the room above by default; in the window's bottom-right corner with the Inspector hidden.
 */
export function chatGeometry(
  viewport: { width: number; height: number },
  workspace: { top: number; right: number; bottom: number; width: number } | null,
  inspector: { left: number; right: number; width: number } | null,
  dockTop: number | null,
  size: ChatSize,
): ChatGeometry {
  const ws = workspace && workspace.width > 0 ? workspace : null;
  const narrow = viewport.width < NARROW_PX;
  const shown = !narrow && inspector && inspector.width > (2 * MIN_WIDTH) / 3 ? inspector : null;
  const right = GAP + (shown ? viewport.width - shown.right : ws ? viewport.width - ws.right : 0);
  // Over the Inspector it sits right on the dock (no strip of Inspector text between them).
  const bottom =
    shown && dockTop !== null ? viewport.height - dockTop : GAP + (ws ? viewport.height - ws.bottom : 0);
  const top = (ws?.top ?? 0) + GAP;
  const maxHeight = Math.max(120, viewport.height - bottom - top);
  const minHeight = Math.min(MIN_HEIGHT, maxHeight);
  const maxWidth = narrow ? viewport.width - 2 * GAP : Math.max(MIN_WIDTH, viewport.width - right - 2 * GAP);
  const width = narrow
    ? maxWidth
    : clamp(size.width ?? (shown ? shown.width - 2 * GAP : HIDDEN_INSPECTOR_WIDTH), MIN_WIDTH, maxWidth);
  const height = size.full
    ? maxHeight
    : clamp(size.height ?? Math.round(maxHeight * 0.75), minHeight, maxHeight);
  return { right, bottom, width, height, minWidth: MIN_WIDTH, maxWidth, minHeight, maxHeight, narrow };
}
