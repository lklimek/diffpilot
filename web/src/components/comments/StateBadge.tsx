import type { PrBadge, PrBadgeView } from "../../api/generated/types";
import { Tooltip } from "../tooltip/Tooltip";

// Plan §3.9 B1–B5: colour, icon and text together; the tooltip lists the facts diffpilot read.
const LOOK: Record<PrBadge, { label: string; icon: string; look: string }> = {
  blocked_failing: { label: "Blocked / failing", icon: "✕", look: "border-high-bd bg-high-bg text-high" },
  github_outdated: { label: "GitHub data outdated", icon: "↻", look: "border-high-bd bg-high-bg text-high" },
  assessment_needed: { label: "Assessment needed", icon: "◌", look: "border-med-bd bg-med-bg text-med" },
  assessing: { label: "Assessment in progress", icon: "◌", look: "border-med-bd bg-med-bg text-med" },
  questions_waiting: { label: "Questions waiting", icon: "?", look: "border-med-bd bg-med-bg text-med" },
  executing: { label: "Executing", icon: "▶", look: "border-chg-bd bg-chg-bg text-chg" },
  waiting_for_bot_round: {
    label: "Waiting for bot round",
    icon: "⌛",
    look: "border-low-bd bg-low-bg text-low",
  },
  ready_to_merge: { label: "Ready to merge", icon: "✓", look: "border-ok bg-ok-bg text-ok" },
};

export const badgeLook = (view: PrBadgeView) =>
  (view.badge && LOOK[view.badge]) || { label: "No state", icon: "–", look: "border-border text-muted" };

/** The tip of a badge: its facts, and what was not read. */
export function badgeTip(view: PrBadgeView): string {
  const facts = view.facts.length ? view.facts.join(" · ") : "";
  const missing = view.missing.length ? `Not known: ${view.missing.join(", ")}.` : "";
  return [facts, missing].filter(Boolean).join(" ") || "No state: not every fact it needs was read.";
}

/** One PR state badge (B1–B5); with unknown inputs and no badge, a muted "No state" naming what is missing. */
export function StateBadge({ view }: { view: PrBadgeView }) {
  const known = badgeLook(view);
  return (
    <Tooltip tip={badgeTip(view)}>
      <span
        tabIndex={0}
        data-state-badge={view.badge ?? "none"}
        className={`inline-flex min-h-6 cursor-help items-center gap-1.5 rounded-full border px-2 text-xs font-semibold whitespace-nowrap ${
          known.look
        }`}
      >
        <span aria-hidden="true">{known.icon}</span>
        {known.label}
      </span>
    </Tooltip>
  );
}
