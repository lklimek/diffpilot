import type { ReactNode } from "react";

const TONE = {
  info: "border-low-bd bg-low-bg",
  warn: "border-med-bd bg-med-bg",
  error: "border-high-bd bg-high-bg",
} as const;

interface BannerProps {
  /** `error` and `warn` are announced as alerts, `info` as a status. */
  tone: keyof typeof TONE;
  /** Bold first sentence. */
  title: string;
  /** Explanation. */
  children?: ReactNode;
  /** Buttons or links at the end of the banner. */
  actions?: ReactNode;
}

/** A message bar with a bold title, an explanation and optional actions (mockup `.banner`). */
export function Banner({ tone, title, children, actions }: BannerProps) {
  return (
    <div
      role={tone === "info" ? "status" : "alert"}
      className={`flex flex-wrap items-start gap-x-2.5 gap-y-1.5 rounded-md border border-l-[5px] px-3.5 py-2.5 ${TONE[tone]}`}
    >
      <strong>{title}</strong>
      {children && <span className="min-w-0 flex-1 basis-80">{children}</span>}
      {actions && <span className="ml-auto flex flex-wrap gap-2">{actions}</span>}
    </div>
  );
}
