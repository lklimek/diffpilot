import type { ChatGeometry, ChatSize } from "../../components/ask/chat/AskChat";

const PHONE_PX = 640;
const GAP = 8;
const MIN_WIDTH = 320;
const MIN_HEIGHT = 240;
const HIDDEN_INSPECTOR_WIDTH = 460;

const clamp = (value: number, min: number, max: number) => Math.min(max, Math.max(min, value));
const finite = (value: number, fallback: number) => (Number.isFinite(value) ? value : fallback);

/** Default placement is above the Inspector dock; explicitly positioned windows use the viewport. */
export function chatGeometry(
  viewport: { width: number; height: number },
  workspace: { top: number; right: number; bottom: number; width: number } | null,
  inspector: { left: number; right: number; width: number } | null,
  dockTop: number | null,
  size: ChatSize,
): ChatGeometry {
  if (viewport.width < PHONE_PX) {
    const height = size.phoneFull ? viewport.height : Math.round(viewport.height / 2);
    return {
      right: 0,
      bottom: 0,
      width: viewport.width,
      height,
      minWidth: viewport.width,
      maxWidth: viewport.width,
      minHeight: Math.round(viewport.height / 2),
      maxHeight: viewport.height,
      narrow: true,
    };
  }

  const ws = workspace && workspace.width > 0 ? workspace : null;
  const shown = inspector && inspector.width > (2 * MIN_WIDTH) / 3 ? inspector : null;
  const maxWidth = viewport.width - 2 * GAP;
  const minWidth = Math.min(MIN_WIDTH, maxWidth);
  const width = clamp(
    finite(size.width ?? NaN, shown ? shown.width - 2 * GAP : HIDDEN_INSPECTOR_WIDTH),
    minWidth,
    maxWidth,
  );
  const position = size.position;
  const moved = position && position !== "bottom-right";
  const custom = typeof position === "object" ? position : undefined;
  const dockBottom =
    shown && dockTop !== null ? viewport.height - dockTop : GAP + (ws ? viewport.height - ws.bottom : 0);
  const defaultTop = Math.max(GAP, (ws?.top ?? 0) + GAP);
  const defaultBottom = Math.max(GAP, dockBottom);
  const baseBottom = moved ? GAP : defaultBottom;
  const room = Math.max(1, viewport.height - 2 * GAP);
  const dockRoom = clamp(viewport.height - defaultTop - defaultBottom, Math.min(120, room), room);
  const maxHeight = moved ? room : dockRoom;
  const minHeight = Math.min(MIN_HEIGHT, maxHeight);
  const height = size.full
    ? maxHeight
    : clamp(finite(size.height ?? NaN, Math.round(dockRoom * 0.75)), minHeight, maxHeight);

  const maxRight = viewport.width - width - GAP;
  const maxBottom = Math.max(GAP, viewport.height - height - GAP);
  const defaultRight = GAP + (shown ? viewport.width - shown.right : ws ? viewport.width - ws.right : 0);
  const right = clamp(
    custom
      ? finite(custom.right, defaultRight)
      : position === "top-left" || position === "bottom-left"
        ? maxRight
        : moved
          ? GAP
          : defaultRight,
    GAP,
    maxRight,
  );
  const bottom = clamp(
    size.full
      ? baseBottom
      : custom
        ? finite(custom.bottom, baseBottom)
        : position === "top-left" || position === "top-right"
          ? maxBottom
          : baseBottom,
    GAP,
    maxBottom,
  );
  return { right, bottom, width, height, minWidth, maxWidth, minHeight, maxHeight, narrow: false };
}
