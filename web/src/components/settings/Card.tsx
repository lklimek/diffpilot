import { useId, type ReactNode } from "react";

/** A titled settings card; the title names the region. */
export function SettingsCard({ title, children }: { title: ReactNode; children: ReactNode }) {
  const id = useId();
  return (
    <section
      aria-labelledby={id}
      className="grid content-start gap-2 rounded-lg border border-border bg-surface px-4 py-3.5"
    >
      <h2 id={id} className="text-base font-semibold">
        {title}
      </h2>
      {children}
    </section>
  );
}
