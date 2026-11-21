import type { ReactNode } from "react";

/** A keyboard key as a chip; inside a control it is decoration (the control's tooltip names the key). */
export function Kbd({ children, decorative = false }: { children: ReactNode; decorative?: boolean }) {
  return (
    <kbd aria-hidden={decorative || undefined} className="rounded border border-b-2 border-border px-1">
      {children}
    </kbd>
  );
}
