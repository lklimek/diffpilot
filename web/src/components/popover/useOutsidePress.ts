import { useEffect, useEffectEvent, type RefObject } from "react";

/** Dismisses a floating panel on a pointer press outside its panel and trigger. */
export function useOutsidePress(
  open: boolean,
  wrapper: RefObject<HTMLElement | null>,
  onDismiss: () => void,
) {
  const dismiss = useEffectEvent(onDismiss);
  useEffect(() => {
    if (!open) return;
    const outside = (event: PointerEvent) => {
      if (!wrapper.current?.contains(event.target as Node)) dismiss();
    };
    document.addEventListener("pointerdown", outside, true);
    return () => document.removeEventListener("pointerdown", outside, true);
  }, [open, wrapper]);
}
