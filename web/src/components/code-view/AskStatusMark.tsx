import type { AskLineStatus } from "../ask/lineStatus";

const STATUS = {
  running: { label: "Answer in progress", glyph: "◷", color: "text-med" },
  answered: { label: "Answer ready", glyph: "✓", color: "text-accent" },
  failed: { label: "No answer", glyph: "!", color: "text-high" },
} satisfies Record<AskLineStatus, { label: string; glyph: string; color: string }>;

export const askStatusLabel = (status: AskLineStatus) => STATUS[status].label;

/** The status is visible on the Ask action, or on its own where code is read-only. */
export function AskStatusMark({ status, inButton = false }: { status: AskLineStatus; inButton?: boolean }) {
  const { label, glyph, color } = STATUS[status];
  return (
    <span
      role={inButton ? undefined : "img"}
      aria-hidden={inButton || undefined}
      aria-label={inButton ? undefined : label}
      title={inButton ? undefined : label}
      data-ask-line-status={inButton ? undefined : status}
      className={`font-sans text-sm font-bold ${color}`}
    >
      {glyph}
    </span>
  );
}
