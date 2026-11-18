import { useId, type ReactNode } from "react";

/** A titled settings card; the title names the region, `help` (a `?` tip) sits next to it. */
export function SettingsCard({
  title,
  help,
  children,
}: {
  title: ReactNode;
  help?: ReactNode;
  children: ReactNode;
}) {
  const id = useId();
  return (
    <section
      aria-labelledby={id}
      className="grid min-w-0 self-start content-start gap-2 rounded-lg border border-border bg-surface px-4 py-3.5"
    >
      <div className="flex items-center gap-2">
        <h2 id={id} className="text-base font-semibold [overflow-wrap:anywhere]">
          {title}
        </h2>
        {help}
      </div>
      {children}
    </section>
  );
}
