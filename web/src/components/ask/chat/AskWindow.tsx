import { useRef, type PointerEvent } from "react";
import { HEAD_BUTTON } from "../../button/button";
import { MenuButton, MenuItem } from "../../menu/MenuButton";
import { Tooltip } from "../../tooltip/Tooltip";
import type { AskChatProps, ChatCorner } from "./AskChat";

const KEY_PX = 20;
const DEFAULT_SIZE = { width: null, height: null, full: false };
const CORNERS: ReadonlyArray<{ value: ChatCorner; label: string }> = [
  { value: "bottom-right", label: "Bottom right — default" },
  { value: "bottom-left", label: "Bottom left" },
  { value: "top-right", label: "Top right" },
  { value: "top-left", label: "Top left" },
];
const MOVE: Record<string, [number, number] | undefined> = {
  ArrowLeft: [1, 0],
  ArrowRight: [-1, 0],
  ArrowUp: [0, 1],
  ArrowDown: [0, -1],
};

export function AskWindowHeader({
  titleId,
  aboutId,
  about,
  geometry,
  size,
  onSize,
  onHide,
}: { titleId: string; aboutId: string } & Pick<
  AskChatProps,
  "about" | "geometry" | "size" | "onSize" | "onHide"
>) {
  const from = useRef<{ x: number; y: number; right: number; bottom: number } | null>(null);
  const { right, bottom, narrow } = geometry;
  const dragHandle = (target: EventTarget) =>
    target instanceof Element && !target.closest("button:not([data-ask-grip]), [role=menu]");
  const move = (x: number, y: number) =>
    onSize({ ...size, position: { right: x, bottom: size.full ? rememberedBottom : y } });
  const rememberedBottom = typeof size.position === "object" ? size.position.bottom : bottom;
  return (
    <div
      className={`flex min-w-0 flex-none items-center gap-1.5 rounded-t-[10px] border-b border-border bg-surface-2 px-2 py-1 ${narrow ? "" : "cursor-move touch-none select-none"}`}
      onPointerDown={(event) => {
        if (narrow || !event.isPrimary || event.button !== 0 || !dragHandle(event.target)) return;
        from.current = { x: event.clientX, y: event.clientY, right, bottom };
        event.currentTarget.setPointerCapture?.(event.pointerId);
      }}
      onPointerMove={(event) => {
        const start = from.current;
        if (!event.isPrimary || !start) return;
        const x = start.x - event.clientX;
        const y = start.y - event.clientY;
        if (x || (y && !size.full)) move(start.right + x, start.bottom + y);
      }}
      onLostPointerCapture={() => (from.current = null)}
      onDoubleClick={(event) => {
        if (!narrow && dragHandle(event.target)) onSize(DEFAULT_SIZE);
      }}
    >
      {!narrow && (
        <Tooltip tip="Drag or use arrow keys to move. Shift moves further. Home resets the position; double-click resets size and position.">
          <button
            type="button"
            data-ask-grip
            aria-label="Move the Ask window: arrow keys; Home resets"
            className="size-6 flex-none cursor-move rounded border-0 bg-transparent text-base text-muted hover:bg-surface"
            onKeyDown={(event) => {
              const direction = MOVE[event.key];
              if (!direction && event.key !== "Home") return;
              event.preventDefault();
              event.stopPropagation();
              if (event.key === "Home") {
                onSize({ ...size, position: undefined });
              } else if (direction && (direction[0] || !size.full)) {
                const step = event.shiftKey ? 50 : 10;
                move(right + step * direction[0], bottom + step * direction[1]);
              }
            }}
          >
            ⠿
          </button>
        </Tooltip>
      )}
      <Tooltip tip="Only you see this conversation. Answers are LLM analysis, not checked; the agent was told not to run code.">
        <h2 id={titleId} tabIndex={0} className="m-0 text-[13.5px] font-bold whitespace-nowrap">
          Ask
        </h2>
      </Tooltip>
      <span id={aboutId} className="sr-only">
        {about} · answers are LLM analysis
      </span>
      <span className="@container min-w-0 flex-1" aria-hidden="true">
        <span className="hidden whitespace-nowrap text-[11.5px] text-muted @min-[120px]:block">
          Private · LLM answers
        </span>
      </span>
      {!narrow && (
        <MenuButton label="Position" trigger="Position ▾" className={HEAD_BUTTON}>
          {CORNERS.map(({ value, label }) => (
            <MenuItem
              key={value}
              label={label}
              selection={{ kind: "radio", checked: (size.position ?? "bottom-right") === value }}
              onSelect={() => onSize({ ...size, position: value === "bottom-right" ? undefined : value })}
            />
          ))}
          <MenuItem label="Reset size and position" onSelect={() => onSize(DEFAULT_SIZE)} />
        </MenuButton>
      )}
      <Tooltip
        tip={
          narrow
            ? "Change the height of the Ask sheet"
            : "Use the full height; click again for your previous size"
        }
      >
        <button
          type="button"
          aria-pressed={narrow ? undefined : size.full}
          onClick={() =>
            onSize(narrow ? { ...size, phoneFull: !size.phoneFull } : { ...size, full: !size.full })
          }
          className={`${HEAD_BUTTON} aria-pressed:border-accent aria-pressed:bg-accent-bg`}
        >
          {narrow ? (size.phoneFull ? "Half" : "Full") : "Full height"}
        </button>
      </Tooltip>
      <Tooltip tip="Hide the chat (Esc). Your draft and answers are kept; the Ask button or a brings it back.">
        <button type="button" aria-label="Hide the Ask chat" onClick={onHide} className={HEAD_BUTTON}>
          Hide
        </button>
      </Tooltip>
    </div>
  );
}

/** One separator of the panel's size: drag, arrow keys, Home / End, Enter or double-click resets. */
export function SizeHandle({
  dimension,
  geometry,
  size,
  onSize,
}: { dimension: "width" | "height" } & Pick<AskChatProps, "geometry" | "size" | "onSize">) {
  const start = useRef<{ x: number; y: number; value: number } | null>(null);
  const height = dimension === "height";
  const [min, max, now] = height
    ? [geometry.minHeight, geometry.maxHeight, geometry.height]
    : [geometry.minWidth, geometry.maxWidth, geometry.width];
  const set = (value: number | null) =>
    onSize(
      height
        ? { ...size, height: value === null ? null : Math.round(value), full: false }
        : { ...size, width: value === null ? null : Math.round(value) },
    );
  return (
    <div
      role="separator"
      aria-orientation={height ? "horizontal" : "vertical"}
      tabIndex={0}
      aria-label={
        height
          ? "Chat height: drag, or use the up and down arrow keys; Home and End go to the limits; Enter or double-click resets"
          : "Chat width: drag, or use the left and right arrow keys; Home and End go to the limits; Enter or double-click resets"
      }
      aria-valuemin={Math.round(min)}
      aria-valuemax={Math.round(max)}
      aria-valuenow={Math.round(now)}
      onPointerDown={(event: PointerEvent<HTMLDivElement>) => {
        start.current = { x: event.clientX, y: event.clientY, value: now };
        event.currentTarget.setPointerCapture?.(event.pointerId);
        event.preventDefault();
      }}
      onPointerMove={(event: PointerEvent<HTMLDivElement>) => {
        const from = start.current;
        if (!from) return;
        set(from.value + (height ? from.y - event.clientY : from.x - event.clientX));
      }}
      onPointerUp={() => (start.current = null)}
      onPointerCancel={() => (start.current = null)}
      onDoubleClick={() => set(null)}
      onKeyDown={(event) => {
        const grow = height ? "ArrowUp" : "ArrowLeft";
        const shrink = height ? "ArrowDown" : "ArrowRight";
        const next =
          event.key === grow
            ? now + KEY_PX
            : event.key === shrink
              ? now - KEY_PX
              : event.key === "Home"
                ? min
                : event.key === "End"
                  ? max
                  : event.key === "Enter"
                    ? null
                    : undefined;
        if (next === undefined) return;
        event.preventDefault();
        set(next === null ? null : Math.min(max, Math.max(min, next)));
      }}
      className={`absolute z-[2] touch-none rounded bg-transparent hover:bg-accent focus-visible:bg-accent ${
        height ? "inset-x-3.5 -top-1 h-2 cursor-ns-resize" : "inset-y-3.5 -left-1 w-2 cursor-ew-resize"
      }`}
    />
  );
}

/** The top-left corner: drags width and height together (the separators are its keyboard alternative). */
export function CornerHandle({ geometry, size, onSize }: Pick<AskChatProps, "geometry" | "size" | "onSize">) {
  const start = useRef<{ x: number; y: number; width: number; height: number } | null>(null);
  const clamp = (value: number, min: number, max: number) => Math.round(Math.min(max, Math.max(min, value)));
  return (
    <div
      data-resize-corner
      aria-hidden="true"
      onPointerDown={(event: PointerEvent<HTMLDivElement>) => {
        start.current = {
          x: event.clientX,
          y: event.clientY,
          width: geometry.width,
          height: geometry.height,
        };
        event.currentTarget.setPointerCapture?.(event.pointerId);
        event.preventDefault();
      }}
      onPointerMove={(event: PointerEvent<HTMLDivElement>) => {
        const from = start.current;
        if (!from) return;
        onSize({
          ...size,
          width: clamp(from.width + from.x - event.clientX, geometry.minWidth, geometry.maxWidth),
          height: clamp(from.height + from.y - event.clientY, geometry.minHeight, geometry.maxHeight),
          full: false,
        });
      }}
      onPointerUp={() => (start.current = null)}
      onPointerCancel={() => (start.current = null)}
      className="absolute -top-1 -left-1 z-[3] size-3.5 cursor-nwse-resize touch-none rounded-tl-[10px] hover:bg-accent"
    />
  );
}
