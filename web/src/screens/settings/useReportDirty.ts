import { useEffect } from "react";

/** Reports the combined dirty state of a page with more than one independent form. */
export function useReportDirty(dirty: boolean, onDirtyChange: (dirty: boolean) => void) {
  useEffect(() => onDirtyChange(dirty), [dirty, onDirtyChange]);
}
