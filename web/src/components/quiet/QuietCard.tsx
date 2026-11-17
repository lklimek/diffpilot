import { useId, type ReactNode } from "react";

interface QuietCardProps {
  /** Heading content. */
  title: ReactNode;
  children: ReactNode;
}

/** Frame of a menu-row block (quiet rows, Not analysed, pending review): heading and the list behind the row. */
export function QuietCard({ title, children }: QuietCardProps) {
  const id = useId();
  return (
    <article
      aria-labelledby={id}
      className="grid gap-2 rounded-lg border border-l-4 border-border border-l-mv-bd bg-surface px-4 py-3 text-[13px]"
    >
      <h2 id={id} className="flex flex-wrap items-center gap-2 text-[17px] font-bold">
        {title}
      </h2>
      {children}
    </article>
  );
}

/** Small grey line under a heading. */
export function Note({ children }: { children: ReactNode }) {
  return <p className="text-[12.5px] text-muted">{children}</p>;
}
