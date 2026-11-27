import { useId, type ReactNode } from "react";

/** A titled settings section; the title names the region, `help` (a `?` tip) sits next to it. */
export function Section({
  title,
  help,
  heading: Heading = "h2",
  children,
}: {
  title: ReactNode;
  help?: ReactNode;
  heading?: "h2" | "h3";
  children: ReactNode;
}) {
  const id = useId();
  return (
    <section
      aria-labelledby={id}
      className="grid min-w-0 self-start content-start gap-3 rounded-lg border border-border bg-surface px-4 py-3.5"
    >
      <div className="flex items-center gap-2">
        <Heading id={id} className="text-base font-semibold [overflow-wrap:anywhere]">
          {title}
        </Heading>
        {help}
      </div>
      {children}
    </section>
  );
}
