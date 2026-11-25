import type { ReactNode } from "react";

/** A labelled form control with space for help alongside it on wide settings pages. */
export function FormRow({
  label,
  htmlFor,
  help,
  children,
}: {
  label: ReactNode;
  htmlFor?: string;
  help?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="grid min-w-0 items-start gap-x-4 gap-y-1 border-t border-border pt-2 first:border-t-0 @min-[520px]/settings:grid-cols-[220px_minmax(0,1fr)] @min-[1100px]/settings:grid-cols-[220px_minmax(0,1fr)_minmax(280px,340px)]">
      <div className="flex flex-wrap items-center gap-2 text-sm font-semibold">
        {htmlFor ? <label htmlFor={htmlFor}>{label}</label> : label}
      </div>
      <div className="grid min-w-0 content-start gap-1">{children}</div>
      {help && (
        <div className="text-xs text-muted @min-[520px]/settings:col-start-2 @min-[1100px]/settings:col-start-3 @min-[1100px]/settings:row-start-1 @min-[1100px]/settings:border-l-2 @min-[1100px]/settings:border-border @min-[1100px]/settings:pl-3">
          {help}
        </div>
      )}
    </div>
  );
}
