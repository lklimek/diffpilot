import { useState, type ReactNode } from "react";

interface AsideSectionProps {
  title: string;
  /** Short summary shown in the header, also while collapsed. */
  summary: ReactNode;
  children: ReactNode;
  /** Open at first. */
  defaultOpen?: boolean;
}

/** A collapsible section of the side panel (`details`/`summary`, mockup `.sec`). */
export function AsideSection({ title, summary, children, defaultOpen = false }: AsideSectionProps) {
  const [open, setOpen] = useState(defaultOpen);
  return (
    <details
      open={open}
      onToggle={(event) => setOpen(event.currentTarget.open)}
      className="group rounded-lg border border-border bg-surface"
    >
      <summary className="flex min-h-9 cursor-pointer list-none flex-wrap items-baseline gap-2 px-3 py-2 before:w-3 before:text-muted before:content-['▸'] group-open:before:content-['▾'] [&::-webkit-details-marker]:hidden">
        <h3 className="text-[13.5px] font-semibold">{title}</h3>
        <span className="ml-auto text-xs text-muted [&_b]:text-text">{summary}</span>
      </summary>
      <div className="px-3 pt-0.5 pb-3 text-[13px]">{open && children}</div>
    </details>
  );
}
