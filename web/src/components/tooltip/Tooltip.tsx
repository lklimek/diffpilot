import { cloneElement, useId, useRef, useState, type ReactElement, type ReactNode } from "react";

interface TooltipProps {
  /** Tooltip content; also the accessible description of the trigger. */
  tip: ReactNode;
  /** The trigger: one focusable element (a non-interactive one needs `tabIndex={0}`). */
  children: ReactElement<{ "aria-describedby"?: string }>;
  /** Classes of the wrapper (layout of the trigger). */
  className?: string;
}

const TIP_WIDTH = 320;

/** Shows `tip` while the trigger is hovered or focused; Escape hides it (WCAG 1.4.13). */
export function Tooltip({ tip, children, className = "" }: TooltipProps) {
  const id = useId();
  const wrapper = useRef<HTMLSpanElement>(null);
  // Fixed position so scroll containers (code view, columns) do not clip the tip.
  const [position, setPosition] = useState<{ top: number; left: number } | null>(null);
  const show = () => {
    const rect = wrapper.current?.getBoundingClientRect();
    if (!rect) return;
    const left = Math.max(4, Math.min(rect.left, window.innerWidth - TIP_WIDTH - 8));
    setPosition({ top: rect.bottom + 4, left });
  };
  const hide = () => setPosition(null);
  return (
    <span
      ref={wrapper}
      className={className}
      onPointerEnter={show}
      onPointerLeave={hide}
      onFocus={show}
      onBlur={hide}
      onKeyDown={(event) => {
        if (event.key === "Escape") hide();
      }}
    >
      {cloneElement(children, { "aria-describedby": id })}
      <span
        id={id}
        role="tooltip"
        hidden={position === null}
        style={position ?? undefined}
        className="fixed z-50 w-max max-w-80 rounded-md bg-text px-2.5 py-1.5 text-left font-sans text-xs font-normal whitespace-normal text-bg normal-case shadow-tip"
      >
        {tip}
      </span>
    </span>
  );
}
