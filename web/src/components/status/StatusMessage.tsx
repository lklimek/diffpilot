import type { ReactNode } from "react";

/** Loading, empty or error state of a screen or block. */
export function StatusMessage({
  kind,
  children,
}: {
  kind: "loading" | "empty" | "error";
  children: ReactNode;
}) {
  if (kind === "error") {
    return (
      <p role="alert" className="rounded-md border border-l-4 border-high-bd bg-high-bg px-3.5 py-2.5">
        {children}
      </p>
    );
  }
  return (
    <p role="status" className="text-muted">
      {children}
    </p>
  );
}
